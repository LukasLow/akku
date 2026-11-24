//! High-level intermediate representation.
//!
//! The parser produces [`Hir`] directly: all flags are already resolved
//! (case-insensitive literals are classes, `.` is a class, `^`/`$` are concrete
//! [`Look`] assertions). Every node caches the properties the compiler needs —
//! most importantly a saturating upper bound on the number of NFA instructions
//! it will expand to, so compile-time blow-ups are rejected *before* any
//! allocation happens.

use super::class::{is_word_char, ClassSet};

/// A zero-width assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Look {
    /// `\A`, or `^` without `(?m)`.
    StartText,
    /// `\z`, or `$` without `(?m)`.
    EndText,
    /// `^` under `(?m)`.
    StartLine,
    /// `$` under `(?m)`.
    EndLine,
    /// `\b` (ASCII).
    WordBoundary,
    /// `\B` (ASCII).
    NotWordBoundary,
}

impl Look {
    /// Evaluates the assertion between `prev` and `next` (`None` = text edge).
    #[inline]
    pub(crate) fn matches(self, prev: Option<char>, next: Option<char>) -> bool {
        match self {
            Look::StartText => prev.is_none(),
            Look::EndText => next.is_none(),
            Look::StartLine => prev.is_none_or(|c| c == '\n'),
            Look::EndLine => next.is_none_or(|c| c == '\n'),
            Look::WordBoundary => is_word(prev) != is_word(next),
            Look::NotWordBoundary => is_word(prev) == is_word(next),
        }
    }
}

#[inline]
fn is_word(c: Option<char>) -> bool {
    c.is_some_and(is_word_char)
}

/// The node payload.
#[derive(Debug)]
pub(crate) enum HirKind {
    /// Matches the empty string.
    Empty,
    /// A single literal character.
    Char(char),
    /// A character class (possibly empty, in which case it never matches).
    Class(ClassSet),
    /// A zero-width assertion.
    Look(Look),
    /// `sub{min,max}`; `max == None` means unbounded.
    Repeat {
        min: u32,
        max: Option<u32>,
        greedy: bool,
        sub: Box<Hir>,
    },
    /// Capturing group `index` (≥ 1).
    Capture { index: u32, sub: Box<Hir> },
    /// Sequence.
    Concat(Vec<Hir>),
    /// Prioritised alternation (leftmost-first).
    Alt(Vec<Hir>),
}

/// Cached node properties.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Props {
    /// Whether the node can match the empty string.
    pub(crate) can_empty: bool,
    /// Group/repetition nesting depth.
    pub(crate) depth: u32,
    /// Saturating upper bound of emitted NFA instructions.
    pub(crate) est: usize,
}

/// A HIR node.
#[derive(Debug)]
pub(crate) struct Hir {
    pub(crate) kind: HirKind,
    pub(crate) props: Props,
}

impl Hir {
    fn leaf(kind: HirKind, can_empty: bool, est: usize) -> Hir {
        Hir {
            kind,
            props: Props {
                can_empty,
                depth: 0,
                est,
            },
        }
    }

    pub(crate) fn empty() -> Hir {
        Hir::leaf(HirKind::Empty, true, 0)
    }

    pub(crate) fn char(c: char) -> Hir {
        Hir::leaf(HirKind::Char(c), false, 1)
    }

    pub(crate) fn class(set: ClassSet) -> Hir {
        if let Some(c) = set.as_single() {
            return Hir::char(c);
        }
        Hir::leaf(HirKind::Class(set), false, 1)
    }

    pub(crate) fn look(look: Look) -> Hir {
        Hir::leaf(HirKind::Look(look), true, 1)
    }

    pub(crate) fn repeat(min: u32, max: Option<u32>, greedy: bool, sub: Hir) -> Hir {
        let copies = max.unwrap_or(min).max(1) as usize;
        let est = sub
            .props
            .est
            .saturating_add(2)
            .saturating_mul(copies)
            .saturating_add(2);
        let props = Props {
            can_empty: min == 0 || sub.props.can_empty,
            depth: sub.props.depth.saturating_add(1),
            est,
        };
        Hir {
            kind: HirKind::Repeat {
                min,
                max,
                greedy,
                sub: Box::new(sub),
            },
            props,
        }
    }

    pub(crate) fn capture(index: u32, sub: Hir) -> Hir {
        let props = Props {
            can_empty: sub.props.can_empty,
            depth: sub.props.depth.saturating_add(1),
            est: sub.props.est.saturating_add(2),
        };
        Hir {
            kind: HirKind::Capture {
                index,
                sub: Box::new(sub),
            },
            props,
        }
    }

    pub(crate) fn concat(mut items: Vec<Hir>) -> Hir {
        match items.len() {
            0 => Hir::empty(),
            1 => items.pop().unwrap_or_else(Hir::empty),
            _ => {
                let props = Props {
                    can_empty: items.iter().all(|h| h.props.can_empty),
                    depth: items.iter().map(|h| h.props.depth).max().unwrap_or(0),
                    est: items
                        .iter()
                        .fold(0usize, |acc, h| acc.saturating_add(h.props.est)),
                };
                Hir {
                    kind: HirKind::Concat(items),
                    props,
                }
            }
        }
    }

    pub(crate) fn alt(mut items: Vec<Hir>) -> Hir {
        match items.len() {
            0 => Hir::empty(),
            1 => items.pop().unwrap_or_else(Hir::empty),
            n => {
                let props = Props {
                    can_empty: items.iter().any(|h| h.props.can_empty),
                    depth: items.iter().map(|h| h.props.depth).max().unwrap_or(0),
                    est: items
                        .iter()
                        .fold(0usize, |acc, h| acc.saturating_add(h.props.est))
                        .saturating_add(n.saturating_mul(2)),
                };
                Hir {
                    kind: HirKind::Alt(items),
                    props,
                }
            }
        }
    }
}

/// Iterative drop so that deeply nested trees cannot overflow the stack.
impl Drop for Hir {
    fn drop(&mut self) {
        match self.kind {
            HirKind::Repeat { .. }
            | HirKind::Capture { .. }
            | HirKind::Concat(_)
            | HirKind::Alt(_) => {}
            _ => return,
        }
        let mut stack = vec![std::mem::replace(&mut self.kind, HirKind::Empty)];
        while let Some(kind) = stack.pop() {
            match kind {
                HirKind::Repeat { mut sub, .. } | HirKind::Capture { mut sub, .. } => {
                    stack.push(std::mem::replace(&mut sub.kind, HirKind::Empty));
                }
                HirKind::Concat(mut items) | HirKind::Alt(mut items) => {
                    for h in &mut items {
                        stack.push(std::mem::replace(&mut h.kind, HirKind::Empty));
                    }
                }
                _ => {}
            }
        }
    }
}
