//! Thompson construction: [`Hir`] → [`Program`].
//!
//! The program is a flat vector of [`Inst`]s. `Split(x, y)` prefers `x`,
//! which encodes greediness and leftmost-first alternation priority. Before
//! anything is emitted, the saturating size estimate cached in the HIR is
//! checked against the configured limit, so repetition bombs fail instantly.

use std::collections::HashMap;

use super::class::ClassSet;
use super::error::ErrorKind;
use super::hir::{Hir, HirKind, Look};

/// Program counter.
pub(crate) type Pc = u32;

/// One NFA instruction.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Inst {
    /// Consume exactly this character.
    Char(char),
    /// Consume one character of class `classes[idx]`.
    Class(u32),
    /// Fork; the first branch has priority.
    Split(Pc, Pc),
    /// Unconditional jump.
    Jmp(Pc),
    /// Record the current position in capture slot `n`.
    Save(u32),
    /// Zero-width assertion.
    Look(Look),
    /// Pattern `id` matched.
    Match(u32),
}

/// Characters that can start a match (a conservative prefilter).
#[derive(Debug, Clone)]
pub(crate) struct FirstSet {
    ascii: [bool; 128],
    non_ascii: bool,
}

impl FirstSet {
    /// Returns the first byte offset `>= at` whose character may start a
    /// match, or `bytes.len()` if there is none.
    #[inline]
    pub(crate) fn skip(&self, bytes: &[u8], mut at: usize) -> usize {
        while let Some(&b) = bytes.get(at) {
            if b < 0x80 {
                if self.ascii[b as usize] {
                    return at;
                }
                at += 1;
            } else {
                if self.non_ascii {
                    return at;
                }
                at += utf8_len(b);
            }
        }
        bytes.len()
    }

    fn mark(&mut self, lo: char, hi: char) {
        let (lo, hi) = (lo as u32, hi as u32);
        if lo < 0x80 {
            for b in lo..=hi.min(0x7F) {
                self.ascii[b as usize] = true;
            }
        }
        if hi >= 0x80 {
            self.non_ascii = true;
        }
    }
}

/// Length of a UTF-8 sequence from its lead byte.
#[inline]
pub(crate) fn utf8_len(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead < 0xE0 {
        2
    } else if lead < 0xF0 {
        3
    } else {
        4
    }
}

/// Maximum number of alphabet classes for which a DFA is built.
const MAX_ALPHABET: usize = 4096;

/// A partition of all scalar values into classes that every instruction,
/// every word-boundary and every line-boundary test treats identically.
#[derive(Debug, Clone)]
pub(crate) struct Alphabet {
    ascii: [u16; 128],
    bounds: Vec<u32>,
    reps: Vec<Option<char>>,
}

impl Alphabet {
    fn new(insts: &[Inst], classes: &[ClassSet]) -> Option<Alphabet> {
        let mut b: Vec<u32> = vec![0, 0x80, 0xD800, 0xE000, 0x0A, 0x0B];
        for (lo, hi) in [('0', '9'), ('A', 'Z'), ('_', '_'), ('a', 'z')] {
            b.push(lo as u32);
            b.push(hi as u32 + 1);
        }
        for inst in insts {
            if let Inst::Char(c) = inst {
                b.push(*c as u32);
                b.push(*c as u32 + 1);
            }
        }
        for class in classes {
            for &(lo, hi) in class.ranges() {
                b.push(lo as u32);
                b.push(hi as u32 + 1);
            }
        }
        b.retain(|&x| x <= char::MAX as u32);
        b.sort_unstable();
        b.dedup();
        if b.len() > MAX_ALPHABET {
            return None;
        }
        let reps = b.iter().map(|&x| char::from_u32(x)).collect();
        let mut alpha = Alphabet {
            ascii: [0; 128],
            bounds: b,
            reps,
        };
        for c in 0u8..128 {
            alpha.ascii[c as usize] = alpha.slow_class(c as u32) as u16;
        }
        Some(alpha)
    }

    fn slow_class(&self, c: u32) -> usize {
        self.bounds.partition_point(|&x| x <= c) - 1
    }

    /// Number of classes (the end-of-input pseudo class has this index).
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.bounds.len()
    }

    /// The class of `c`.
    #[inline]
    pub(crate) fn class_of(&self, c: char) -> usize {
        let c = c as u32;
        if c < 0x80 {
            self.ascii[c as usize] as usize
        } else {
            self.slow_class(c)
        }
    }

    /// A representative character of class `cls` (`None` for the surrogate gap).
    #[inline]
    pub(crate) fn rep(&self, cls: usize) -> Option<char> {
        self.reps.get(cls).copied().flatten()
    }
}

/// A compiled program plus the metadata the engines need.
#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) insts: Vec<Inst>,
    pub(crate) classes: Vec<ClassSet>,
    /// Number of capture slots (2 per group, including group 0).
    pub(crate) slot_count: usize,
    /// Every match must start at text offset 0.
    pub(crate) anchored: bool,
    /// A literal every match starts with.
    pub(crate) prefix: Option<String>,
    /// The pattern *is* exactly this literal (no assertions, no classes);
    /// searches without inner captures reduce to substring search.
    pub(crate) literal: Option<String>,
    /// Characters that can start a match (absent when the empty string can match).
    pub(crate) first: Option<FirstSet>,
    /// The DFA alphabet (absent if too many classes).
    pub(crate) alphabet: Option<Alphabet>,
}

fn check_size(insts: usize, slots: usize, limit: usize) -> Result<(), ErrorKind> {
    let per_inst = std::mem::size_of::<Inst>()
        .saturating_add(slots.saturating_mul(std::mem::size_of::<usize>()));
    if insts.saturating_mul(per_inst) > limit {
        Err(ErrorKind::SizeLimitExceeded { limit })
    } else {
        Ok(())
    }
}

/// Compiles a single pattern: `Save(0) body Save(1) Match(0)`.
pub(crate) fn compile(
    hir: &Hir,
    slot_count: usize,
    size_limit: usize,
) -> Result<Program, ErrorKind> {
    check_size(hir.props.est.saturating_add(3), slot_count, size_limit)?;
    let mut c = Compiler::default();
    c.emit(Inst::Save(0));
    c.c(hir);
    c.emit(Inst::Save(1));
    c.emit(Inst::Match(0));
    let mut prefix = String::new();
    literal_prefix(hir, &mut prefix);
    let literal = is_pure_literal(hir).then(|| prefix.clone());
    let prefix = (!prefix.is_empty()).then_some(prefix);
    let mut prog = c.finish(slot_count, is_anchored(hir), prefix, size_limit)?;
    prog.literal = literal;
    Ok(prog)
}

/// Whether `hir` matches exactly one fixed string (captures allowed).
/// Iterative, so it is safe on arbitrarily long concatenations.
fn is_pure_literal(hir: &Hir) -> bool {
    let mut stack = vec![hir];
    while let Some(h) = stack.pop() {
        match &h.kind {
            HirKind::Empty | HirKind::Char(_) => {}
            HirKind::Capture { sub, .. } => stack.push(sub),
            HirKind::Concat(items) => stack.extend(items.iter()),
            _ => return false,
        }
    }
    true
}

/// Compiles several patterns into one program with `Match(i)` per pattern.
pub(crate) fn compile_set(hirs: &[Hir], size_limit: usize) -> Result<Program, ErrorKind> {
    let est = hirs.iter().fold(0usize, |acc, h| {
        acc.saturating_add(h.props.est).saturating_add(4)
    });
    check_size(est, 0, size_limit)?;
    let mut c = Compiler::default();
    for (i, hir) in hirs.iter().enumerate() {
        if i + 1 < hirs.len() {
            let split = c.emit(Inst::Split(0, 0));
            let body = c.pc();
            c.c(hir);
            c.emit(Inst::Match(i as u32));
            let next = c.pc();
            c.insts[split as usize] = Inst::Split(body, next);
        } else {
            c.c(hir);
            c.emit(Inst::Match(i as u32));
        }
    }
    let anchored = !hirs.is_empty() && hirs.iter().all(is_anchored);
    c.finish(0, anchored, None, size_limit)
}

#[derive(Default)]
struct Compiler {
    insts: Vec<Inst>,
    classes: Vec<ClassSet>,
    class_ids: HashMap<Vec<(char, char)>, u32>,
}

impl Compiler {
    fn pc(&self) -> Pc {
        self.insts.len() as Pc
    }

    fn emit(&mut self, inst: Inst) -> Pc {
        self.insts.push(inst);
        self.pc() - 1
    }

    fn set_split(&mut self, at: Pc, prefer: Pc, other: Pc, greedy: bool) {
        self.insts[at as usize] = if greedy {
            Inst::Split(prefer, other)
        } else {
            Inst::Split(other, prefer)
        };
    }

    fn class_id(&mut self, set: &ClassSet) -> u32 {
        if let Some(&id) = self.class_ids.get(set.ranges()) {
            return id;
        }
        let id = self.classes.len() as u32;
        self.classes.push(set.clone());
        self.class_ids.insert(set.ranges().to_vec(), id);
        id
    }

    /// Recursion depth is bounded by the parser's nesting limit (≤ 1000).
    fn c(&mut self, hir: &Hir) {
        match &hir.kind {
            HirKind::Empty => {}
            HirKind::Char(ch) => {
                self.emit(Inst::Char(*ch));
            }
            HirKind::Class(set) => {
                let id = self.class_id(set);
                self.emit(Inst::Class(id));
            }
            HirKind::Look(look) => {
                self.emit(Inst::Look(*look));
            }
            HirKind::Capture { index, sub } => {
                self.emit(Inst::Save(index * 2));
                self.c(sub);
                self.emit(Inst::Save(index * 2 + 1));
            }
            HirKind::Concat(items) => {
                for item in items {
                    self.c(item);
                }
            }
            HirKind::Alt(items) => self.c_alt(items),
            HirKind::Repeat {
                min,
                max,
                greedy,
                sub,
            } => self.c_repeat(*min, *max, *greedy, sub),
        }
    }

    fn c_alt(&mut self, items: &[Hir]) {
        let mut jumps = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            if i + 1 < items.len() {
                let split = self.emit(Inst::Split(0, 0));
                let body = self.pc();
                self.c(item);
                jumps.push(self.emit(Inst::Jmp(0)));
                let next = self.pc();
                self.insts[split as usize] = Inst::Split(body, next);
            } else {
                self.c(item);
            }
        }
        let end = self.pc();
        for j in jumps {
            self.insts[j as usize] = Inst::Jmp(end);
        }
    }

    fn c_plus(&mut self, sub: &Hir, greedy: bool) {
        let body = self.pc();
        self.c(sub);
        let split = self.emit(Inst::Split(0, 0));
        let end = self.pc();
        self.set_split(split, body, end, greedy);
    }

    fn c_star(&mut self, sub: &Hir, greedy: bool) {
        let split = self.emit(Inst::Split(0, 0));
        let body = self.pc();
        if sub.props.can_empty {
            // `(?:e+)?` gives PCRE-compatible captures for `(a*)*`.
            self.c_plus(sub, greedy);
        } else {
            self.c(sub);
            self.emit(Inst::Jmp(split));
        }
        let end = self.pc();
        self.set_split(split, body, end, greedy);
    }

    fn c_repeat(&mut self, min: u32, max: Option<u32>, greedy: bool, sub: &Hir) {
        match max {
            None if min == 0 => self.c_star(sub, greedy),
            None => {
                for _ in 1..min {
                    self.c(sub);
                }
                self.c_plus(sub, greedy);
            }
            Some(max) => {
                for _ in 0..min {
                    self.c(sub);
                }
                let mut holes = Vec::with_capacity((max - min) as usize);
                for _ in min..max {
                    let split = self.emit(Inst::Split(0, 0));
                    holes.push((split, self.pc()));
                    self.c(sub);
                }
                let end = self.pc();
                for (split, body) in holes {
                    self.set_split(split, body, end, greedy);
                }
            }
        }
    }

    fn finish(
        self,
        slot_count: usize,
        anchored: bool,
        prefix: Option<String>,
        size_limit: usize,
    ) -> Result<Program, ErrorKind> {
        let class_bytes: usize = self.classes.iter().map(ClassSet::heap_bytes).sum();
        check_size(
            self.insts.len(),
            slot_count,
            size_limit.saturating_sub(class_bytes),
        )?;
        if class_bytes > size_limit {
            return Err(ErrorKind::SizeLimitExceeded { limit: size_limit });
        }
        let first = first_set(&self.insts, &self.classes);
        let alphabet = Alphabet::new(&self.insts, &self.classes);
        Ok(Program {
            insts: self.insts,
            classes: self.classes,
            slot_count,
            anchored,
            prefix,
            literal: None,
            first,
            alphabet,
        })
    }
}

/// Every match starts with `\A`.
fn is_anchored(hir: &Hir) -> bool {
    match &hir.kind {
        HirKind::Look(Look::StartText) => true,
        HirKind::Capture { sub, .. } => is_anchored(sub),
        HirKind::Repeat { min, sub, .. } => *min >= 1 && is_anchored(sub),
        HirKind::Concat(items) => items.first().is_some_and(is_anchored),
        HirKind::Alt(items) => items.iter().all(is_anchored),
        _ => false,
    }
}

/// Appends the case-sensitive literal every match begins with; returns
/// whether the whole node was literal (so the caller may continue).
fn literal_prefix(hir: &Hir, out: &mut String) -> bool {
    match &hir.kind {
        HirKind::Empty | HirKind::Look(_) => true,
        HirKind::Char(c) => {
            out.push(*c);
            true
        }
        HirKind::Capture { sub, .. } => literal_prefix(sub, out),
        HirKind::Concat(items) => items.iter().all(|item| literal_prefix(item, out)),
        HirKind::Repeat { min, sub, .. } if *min >= 1 => {
            literal_prefix(sub, out);
            false
        }
        _ => false,
    }
}

/// Conservative set of first characters; `None` if a match may be empty or
/// the set is useless.
fn first_set(insts: &[Inst], classes: &[ClassSet]) -> Option<FirstSet> {
    let mut fs = FirstSet {
        ascii: [false; 128],
        non_ascii: false,
    };
    if insts.is_empty() {
        return None;
    }
    let mut seen = vec![false; insts.len()];
    let mut stack = vec![0 as Pc];
    while let Some(pc) = stack.pop() {
        let i = pc as usize;
        if seen[i] {
            continue;
        }
        seen[i] = true;
        match insts[i] {
            Inst::Char(c) => fs.mark(c, c),
            Inst::Class(k) => {
                for &(lo, hi) in classes[k as usize].ranges() {
                    fs.mark(lo, hi);
                }
            }
            Inst::Split(a, b) => {
                stack.push(b);
                stack.push(a);
            }
            Inst::Jmp(t) => stack.push(t),
            Inst::Save(_) | Inst::Look(_) => stack.push(pc + 1),
            Inst::Match(_) => return None,
        }
    }
    if fs.non_ascii && fs.ascii.iter().all(|&b| b) {
        return None;
    }
    Some(fs)
}
