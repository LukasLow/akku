//! The PikeVM: a breadth-first Thompson NFA simulation with capture slots.
//!
//! Every instruction is visited at most once per input position (deduplicated
//! by a sparse set), so a search takes `O(n · m)` steps for a haystack of `n`
//! characters and a program of `m` instructions — regardless of the pattern.
//! Threads are kept in priority order, which yields leftmost-first (Perl)
//! semantics: when a thread matches, all lower-priority threads are cut.
//!
//! The epsilon closure uses an explicit stack (never recursion), with
//! `Restore` frames to undo capture writes when backtracking out of a branch
//! of the closure.

use super::nfa::{Inst, Pc, Program};

/// Sentinel for an unset capture slot.
pub(crate) const NONE: usize = usize::MAX;

/// A sparse set of program counters with O(1) insert, lookup and clear.
#[derive(Debug, Clone, Default)]
pub(crate) struct SparseSet {
    dense: Vec<Pc>,
    sparse: Vec<Pc>,
    len: usize,
}

impl SparseSet {
    pub(crate) fn new(capacity: usize) -> SparseSet {
        SparseSet {
            dense: vec![0; capacity],
            sparse: vec![0; capacity],
            len: 0,
        }
    }

    #[inline]
    pub(crate) fn capacity(&self) -> usize {
        self.dense.len()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Inserts `pc`; returns `false` if it was already present.
    #[inline]
    pub(crate) fn insert(&mut self, pc: Pc) -> bool {
        let i = self.sparse[pc as usize] as usize;
        if i < self.len && self.dense[i] == pc {
            return false;
        }
        self.dense[self.len] = pc;
        self.sparse[pc as usize] = self.len as Pc;
        self.len += 1;
        true
    }

    #[inline]
    pub(crate) fn get(&self, i: usize) -> Pc {
        self.dense[i]
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }
}

/// A thread list: the set of live program counters plus their capture slots.
#[derive(Debug, Clone, Default)]
struct Threads {
    set: SparseSet,
    slots: Vec<usize>,
    stride: usize,
}

impl Threads {
    fn prepare(&mut self, capacity: usize, stride: usize) {
        if self.set.capacity() != capacity {
            self.set = SparseSet::new(capacity);
        }
        self.set.clear();
        self.stride = stride;
        let need = capacity * stride;
        if self.slots.len() < need {
            self.slots.resize(need, NONE);
        }
    }

    #[inline]
    fn slots(&self, pc: usize) -> &[usize] {
        &self.slots[pc * self.stride..(pc + 1) * self.stride]
    }
}

#[derive(Debug, Clone, Copy)]
enum Frame {
    Explore(Pc),
    Restore(usize, usize),
}

/// Reusable per-search scratch memory.
#[derive(Debug, Clone, Default)]
pub(crate) struct PikeCache {
    clist: Threads,
    nlist: Threads,
    stack: Vec<Frame>,
    scratch: Vec<usize>,
    /// Slots of the last reported match.
    pub(crate) result: Vec<usize>,
}

impl PikeCache {
    pub(crate) fn new() -> PikeCache {
        PikeCache::default()
    }
}

/// Decodes the character starting at byte `i` of valid UTF-8 `b`.
#[inline]
pub(crate) fn decode(b: &[u8], i: usize) -> Option<(char, usize)> {
    let x = *b.get(i)?;
    if x < 0x80 {
        return Some((x as char, 1));
    }
    let (n, mut cp) = if x < 0xE0 {
        (2, u32::from(x & 0x1F))
    } else if x < 0xF0 {
        (3, u32::from(x & 0x0F))
    } else {
        (4, u32::from(x & 0x07))
    };
    for k in 1..n {
        cp = (cp << 6) | u32::from(b[i + k] & 0x3F);
    }
    Some((char::from_u32(cp).unwrap_or('\u{FFFD}'), n))
}

/// The character before byte offset `at` (`None` at the start of the text).
#[inline]
pub(crate) fn prev_char(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

/// Runs the PikeVM from `start`.
///
/// * `nslots` — number of capture slots to track (0, 2 or all).
/// * `set` — set mode: record every matching pattern id, never cut.
/// * `earliest` — return as soon as any match is known (for `is_match`).
///
/// On a (non-set, non-earliest) match the slots are left in `cache.result`.
pub(crate) fn search(
    prog: &Program,
    cache: &mut PikeCache,
    text: &str,
    start: usize,
    nslots: usize,
    mut set: Option<&mut [bool]>,
    earliest: bool,
) -> bool {
    if prog.insts.is_empty() || (prog.anchored && start > 0) {
        return false;
    }
    if let (Some(lit), None, true) = (&prog.literal, &set, nslots <= 2) {
        let Some(i) = text[start..].find(lit.as_str()) else {
            return false;
        };
        if !earliest {
            let s = start + i;
            cache.result.clear();
            cache
                .result
                .extend_from_slice(&[s, s + lit.len()][..nslots]);
        }
        return true;
    }
    let bytes = text.as_bytes();
    let cap = prog.insts.len();
    let PikeCache {
        clist,
        nlist,
        stack,
        scratch,
        result,
    } = cache;
    clist.prepare(cap, nslots);
    nlist.prepare(cap, nslots);
    scratch.clear();
    scratch.resize(nslots, NONE);
    let set_mode = set.is_some();
    let mut remaining = set
        .as_deref()
        .map_or(0, |s| s.iter().filter(|m| !**m).count());
    if set_mode && remaining == 0 {
        return true;
    }
    let mut matched = false;
    let mut at = start;
    let mut prev = prev_char(text, at);
    let mut cur = decode(bytes, at);
    loop {
        let may_seed = if prog.anchored {
            at == 0
        } else {
            set_mode || !matched
        };
        if may_seed {
            if clist.set.is_empty() && !prog.anchored {
                let target = if let Some(p) = &prog.prefix {
                    text[at..].find(p.as_str()).map(|i| at + i)
                } else if let Some(fs) = &prog.first {
                    Some(fs.skip(bytes, at))
                } else {
                    Some(at)
                };
                match target {
                    None => break,
                    Some(to) if to != at => {
                        at = to;
                        prev = prev_char(text, at);
                        cur = decode(bytes, at);
                    }
                    Some(_) => {}
                }
            }
            scratch.fill(NONE);
            add_thread(prog, clist, stack, scratch, 0, at, prev, cur.map(|x| x.0));
        }
        // With no live threads we may only stop when no future position can
        // seed a new one; otherwise a failed seed (e.g. `\b` or `$` not
        // holding here) would end an unanchored search too early.
        if clist.set.is_empty() && !(may_seed && !prog.anchored) {
            break;
        }
        let (c, clen) = match cur {
            Some((c, l)) => (Some(c), l),
            None => (None, 0),
        };
        let next_at = at + clen;
        let nxt = if c.is_some() {
            decode(bytes, next_at)
        } else {
            None
        };
        let next_c = nxt.map(|x| x.0);
        let mut i = 0;
        while i < clist.set.len() {
            let pc = clist.set.get(i) as usize;
            i += 1;
            let step = match prog.insts[pc] {
                Inst::Char(x) => c == Some(x),
                Inst::Class(k) => c.is_some_and(|ch| prog.classes[k as usize].contains(ch)),
                Inst::Match(id) => {
                    matched = true;
                    if let Some(s) = set.as_deref_mut() {
                        let id = id as usize;
                        if !s[id] {
                            s[id] = true;
                            remaining -= 1;
                            if remaining == 0 {
                                return true;
                            }
                        }
                        continue;
                    }
                    if earliest {
                        return true;
                    }
                    result.clear();
                    result.extend_from_slice(clist.slots(pc));
                    // Leftmost-first: drop every lower-priority thread.
                    break;
                }
                _ => false,
            };
            if step {
                scratch.copy_from_slice(clist.slots(pc));
                add_thread(
                    prog,
                    nlist,
                    stack,
                    scratch,
                    pc as Pc + 1,
                    next_at,
                    c,
                    next_c,
                );
            }
        }
        std::mem::swap(clist, nlist);
        nlist.set.clear();
        if c.is_none() {
            break;
        }
        at = next_at;
        prev = c;
        cur = nxt;
    }
    matched
}

/// Follows epsilon transitions from `pc0` at position `at`, adding every
/// reachable consuming/match instruction to `list` with a copy of `slots`.
#[allow(clippy::too_many_arguments)]
#[inline]
fn add_thread(
    prog: &Program,
    list: &mut Threads,
    stack: &mut Vec<Frame>,
    slots: &mut [usize],
    pc0: Pc,
    at: usize,
    prev: Option<char>,
    next: Option<char>,
) {
    stack.push(Frame::Explore(pc0));
    while let Some(frame) = stack.pop() {
        let mut pc = match frame {
            Frame::Restore(i, v) => {
                slots[i] = v;
                continue;
            }
            Frame::Explore(pc) => pc,
        };
        loop {
            if !list.set.insert(pc) {
                break;
            }
            match prog.insts[pc as usize] {
                Inst::Jmp(t) => pc = t,
                Inst::Split(a, b) => {
                    stack.push(Frame::Explore(b));
                    pc = a;
                }
                Inst::Save(n) => {
                    let n = n as usize;
                    if n < slots.len() {
                        stack.push(Frame::Restore(n, slots[n]));
                        slots[n] = at;
                    }
                    pc += 1;
                }
                Inst::Look(look) => {
                    if !look.matches(prev, next) {
                        break;
                    }
                    pc += 1;
                }
                Inst::Char(_) | Inst::Class(_) | Inst::Match(_) => {
                    let stride = slots.len();
                    if stride > 0 {
                        let base = pc as usize * stride;
                        list.slots[base..base + stride].copy_from_slice(slots);
                    }
                    break;
                }
            }
        }
    }
}
