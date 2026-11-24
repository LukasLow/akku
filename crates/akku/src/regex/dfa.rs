//! A lazy (on-demand) DFA that answers "is there any match at or after
//! `start`?" in a single forward pass.
//!
//! States are sets of NFA program counters *before* their epsilon closure,
//! paired with a look-behind context (start of text / after `\n` / after a
//! word char / other). The closure is computed during a transition, when the
//! next character's class is known, so `$`, `\b` and friends are resolved
//! exactly. Because the question is only "does a match exist", the state sets
//! are unordered (sorted, deduplicated) — priorities do not matter.
//!
//! Transitions are cached in a flat table bounded by `dfa_size_limit`. When
//! the budget is exceeded the cache is flushed; if flushes keep happening with
//! little progress the search gives up (returns `None`) and the caller falls
//! back to the PikeVM. Either way the search stays linear.

use std::collections::HashMap;

use super::class::is_word_char;
use super::nfa::{Alphabet, Inst, Pc, Program};
use super::pikevm::{decode, prev_char, SparseSet};

const UNKNOWN: u32 = u32::MAX;
const DEAD: u32 = u32::MAX - 1;
const MATCH: u32 = u32::MAX - 2;

/// Look-behind contexts.
const CTX_START: u8 = 0;
const CTX_NEWLINE: u8 = 1;
const CTX_WORD: u8 = 2;
const CTX_OTHER: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    ctx: u8,
    core: Box<[Pc]>,
}

/// Per-search reusable DFA state cache.
#[derive(Debug, Clone)]
pub(crate) struct DfaCache {
    map: HashMap<Key, u32>,
    states: Vec<Key>,
    trans: Vec<u32>,
    stride: usize,
    mem: usize,
    limit: usize,
    seen: SparseSet,
    stack: Vec<Pc>,
    consuming: Vec<Pc>,
    core: Vec<Pc>,
}

enum Target {
    Match,
    Dead,
    State(Key),
}

struct Progress {
    since_clear: usize,
    bad_clears: u32,
}

fn ctx_of(c: Option<char>) -> u8 {
    match c {
        None => CTX_START,
        Some('\n') => CTX_NEWLINE,
        Some(c) if is_word_char(c) => CTX_WORD,
        Some(_) => CTX_OTHER,
    }
}

fn ctx_prev(ctx: u8) -> Option<char> {
    match ctx {
        CTX_START => None,
        CTX_NEWLINE => Some('\n'),
        CTX_WORD => Some('a'),
        _ => Some(' '),
    }
}

impl DfaCache {
    /// Creates a cache for `prog`, or `None` if the program has no alphabet.
    pub(crate) fn new(prog: &Program, limit: usize) -> Option<DfaCache> {
        let alpha = prog.alphabet.as_ref()?;
        Some(DfaCache {
            map: HashMap::new(),
            states: Vec::new(),
            trans: Vec::new(),
            stride: alpha.len() + 1,
            mem: 0,
            limit,
            seen: SparseSet::new(prog.insts.len()),
            stack: Vec::new(),
            consuming: Vec::new(),
            core: Vec::new(),
        })
    }

    fn state_bytes(&self, key: &Key) -> usize {
        self.stride * 4 + key.core.len() * 8 + 96
    }

    fn intern(&mut self, key: Key) -> u32 {
        if let Some(&id) = self.map.get(&key) {
            return id;
        }
        let id = self.states.len() as u32;
        self.mem += self.state_bytes(&key);
        self.trans.resize(self.trans.len() + self.stride, UNKNOWN);
        self.states.push(key.clone());
        self.map.insert(key, id);
        id
    }

    fn clear(&mut self) {
        self.map.clear();
        self.states.clear();
        self.trans.clear();
        self.mem = 0;
    }

    fn step(&mut self, prog: &Program, alpha: &Alphabet, key: &Key, cls: usize) -> Target {
        let prev = ctx_prev(key.ctx);
        let eoi = cls == alpha.len();
        let next = if eoi { None } else { alpha.rep(cls) };
        if !eoi && next.is_none() {
            return Target::Dead;
        }
        self.seen.clear();
        self.consuming.clear();
        self.stack.clear();
        self.stack.extend(key.core.iter().rev());
        while let Some(pc) = self.stack.pop() {
            if !self.seen.insert(pc) {
                continue;
            }
            match prog.insts[pc as usize] {
                Inst::Jmp(t) => self.stack.push(t),
                Inst::Split(a, b) => {
                    self.stack.push(b);
                    self.stack.push(a);
                }
                Inst::Save(_) => self.stack.push(pc + 1),
                Inst::Look(look) => {
                    if look.matches(prev, next) {
                        self.stack.push(pc + 1);
                    }
                }
                Inst::Match(_) => return Target::Match,
                Inst::Char(_) | Inst::Class(_) => self.consuming.push(pc),
            }
        }
        let Some(c) = next else {
            return Target::Dead;
        };
        self.core.clear();
        for &pc in &self.consuming {
            let ok = match prog.insts[pc as usize] {
                Inst::Char(x) => x == c,
                Inst::Class(k) => prog.classes[k as usize].contains(c),
                _ => false,
            };
            if ok {
                self.core.push(pc + 1);
            }
        }
        if !prog.anchored {
            self.core.push(0);
        }
        if self.core.is_empty() {
            return Target::Dead;
        }
        self.core.sort_unstable();
        self.core.dedup();
        Target::State(Key {
            ctx: ctx_of(Some(c)),
            core: self.core.as_slice().into(),
        })
    }

    fn compute(
        &mut self,
        prog: &Program,
        alpha: &Alphabet,
        s: u32,
        cls: usize,
        progress: &mut Progress,
    ) -> Option<u32> {
        let key = self.states[s as usize].clone();
        let t = match self.step(prog, alpha, &key, cls) {
            Target::Match => MATCH,
            Target::Dead => DEAD,
            Target::State(next) => {
                if !self.map.contains_key(&next) && self.mem + self.state_bytes(&next) > self.limit
                {
                    if progress.since_clear < 10 * self.states.len() {
                        progress.bad_clears += 1;
                        if progress.bad_clears >= 3 {
                            return None;
                        }
                    }
                    progress.since_clear = 0;
                    self.clear();
                    let s2 = self.intern(key);
                    let t = self.intern(next);
                    self.trans[s2 as usize * self.stride + cls] = t;
                    return Some(t);
                }
                self.intern(next)
            }
        };
        self.trans[s as usize * self.stride + cls] = t;
        Some(t)
    }
}

/// Returns whether `prog` matches anywhere in `text[start..]` (with look-behind
/// context from `text[..start]`), or `None` if the DFA gave up.
pub(crate) fn is_match(
    prog: &Program,
    cache: &mut DfaCache,
    text: &str,
    start: usize,
) -> Option<bool> {
    let alpha = prog.alphabet.as_ref()?;
    if prog.insts.is_empty() || (prog.anchored && start > 0) {
        return Some(false);
    }
    if let Some(lit) = &prog.literal {
        return Some(text[start..].contains(lit.as_str()));
    }
    let mut at = start;
    if let Some(p) = &prog.prefix {
        match text[start..].find(p.as_str()) {
            Some(i) => at = start + i,
            None => return Some(false),
        }
    }
    let bytes = text.as_bytes();
    let mut progress = Progress {
        since_clear: 0,
        bad_clears: 0,
    };
    let mut s = cache.intern(Key {
        ctx: ctx_of(prev_char(text, at)),
        core: Box::new([0]),
    });
    let eoi = alpha.len();
    loop {
        let (cls, step) = match decode(bytes, at) {
            Some((c, len)) => (alpha.class_of(c), len),
            None => (eoi, 0),
        };
        let mut t = cache.trans[s as usize * cache.stride + cls];
        if t == UNKNOWN {
            t = cache.compute(prog, alpha, s, cls, &mut progress)?;
        }
        if t == MATCH {
            return Some(true);
        }
        if t == DEAD || step == 0 {
            return Some(false);
        }
        s = t;
        at += step;
        progress.since_clear += 1;
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::regex::pikevm::{search, PikeCache};
    use crate::regex::RegexBuilder;

    /// The DFA and the PikeVM must agree on `is_match` everywhere.
    #[test]
    fn dfa_agrees_with_pikevm() {
        let patterns = [
            "a",
            "abc",
            "a|b",
            "(a+)+b",
            "^abc$",
            "(?m)^b$",
            r"\bfoo\b",
            r"\Bo\B",
            "a*",
            "",
            r"\d+\s\w+",
            "[^a-c]x",
            "(?i)straße",
            "(?s).{3}",
            r"x\z",
            r"\Ay",
            "é+",
            "(?i)k",
            "(a|ab)(c|bcd)(d*)",
            r"^(\w+\s?)*$",
            "[α-ω]+$",
            "$",
            "^",
        ];
        let hays = [
            "", "a", "b", "abc", "xabcx", "a\nb\nc", "foo", "a foo.", "foo", "aaab", "12 ab", "dx",
            "ax", "STRASSE", "\u{212A}", "x", "yx", "ééé", "abcd", "ab cd !", "αβγ",
        ];
        for p in patterns {
            for limit in [usize::MAX, 0] {
                let re = RegexBuilder::new(p).dfa_size_limit(limit).build().unwrap();
                let prog = &re.inner.prog;
                for h in hays {
                    for start in (0..=h.len()).filter(|&i| h.is_char_boundary(i)) {
                        let mut pc = PikeCache::new();
                        let expect = search(prog, &mut pc, h, start, 0, None, true);
                        let mut dc = DfaCache::new(prog, limit).unwrap();
                        if let Some(got) = is_match(prog, &mut dc, h, start) {
                            assert_eq!(got, expect, "pattern {p:?} hay {h:?} start {start}");
                        }
                    }
                }
            }
        }
    }
}
