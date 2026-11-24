//! Pattern parser: pattern text → [`Hir`].
//!
//! The parser is iterative — groups are tracked on an explicit heap stack —
//! so pattern nesting can never overflow the call stack. Flags are resolved
//! while parsing: `(?flags)` lasts until the end of the enclosing group
//! (including later alternatives), `(?flags:...)` only inside its group.

use std::collections::HashSet;

use super::builder::Config;
use super::class::{ClassSet, Perl};
use super::error::{Error, ErrorKind, Span};
use super::fold;
use super::hir::{Hir, Look};

/// Hard upper bound for `m` and `n` in `{m,n}` (as in RE2).
pub(crate) const MAX_REPEAT: u32 = 1000;

/// Parser output.
pub(crate) struct Parsed {
    pub(crate) hir: Hir,
    /// Group names indexed by group number; index 0 is the whole match.
    pub(crate) names: Vec<Option<Box<str>>>,
}

/// Parses `pattern` under `config`.
pub(crate) fn parse(pattern: &str, config: &Config) -> Result<Parsed, Error> {
    if pattern.len() > config.pattern_len_limit {
        return Err(Error::new(
            ErrorKind::PatternTooLong {
                limit: config.pattern_len_limit,
            },
            None,
            pattern,
        ));
    }
    let mut p = Parser {
        pat: pattern,
        pos: 0,
        flags: Flags {
            i: config.case_insensitive,
            m: config.multi_line,
            s: config.dot_matches_new_line,
        },
        nest_limit: config.nest_limit,
        names: vec![None],
        name_set: HashSet::new(),
        stack: Vec::new(),
        alts: Vec::new(),
        concat: Vec::new(),
        last: Last::Nothing,
    };
    match p.run() {
        Ok(hir) => Ok(Parsed {
            hir,
            names: p.names,
        }),
        Err((kind, span)) => Err(Error::new(kind, Some(span), pattern)),
    }
}

type PResult<T> = Result<T, (ErrorKind, Span)>;

fn err<T>(kind: ErrorKind, start: usize, end: usize) -> PResult<T> {
    Err((kind, Span { start, end }))
}

#[derive(Debug, Clone, Copy)]
struct Flags {
    i: bool,
    m: bool,
    s: bool,
}

/// What the last item of the current concatenation was (for repetition checks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    Nothing,
    Atom,
    Repeated,
}

enum GroupKind {
    Capture(u32),
    NonCapture,
}

struct Frame {
    kind: GroupKind,
    open: usize,
    flags: Flags,
    alts: Vec<Hir>,
    concat: Vec<Hir>,
}

enum Escape {
    Char(char),
    Class(ClassSet),
    Look(Look),
}

struct Parser<'p> {
    pat: &'p str,
    pos: usize,
    flags: Flags,
    nest_limit: u32,
    names: Vec<Option<Box<str>>>,
    name_set: HashSet<&'p str>,
    stack: Vec<Frame>,
    alts: Vec<Hir>,
    concat: Vec<Hir>,
    last: Last,
}

impl<'p> Parser<'p> {
    fn peek(&self) -> Option<char> {
        self.pat[self.pos..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        self.pat[self.pos..].chars().nth(1)
    }

    fn char_end(&self) -> usize {
        self.pos + self.peek().map_or(0, char::len_utf8)
    }

    fn run(&mut self) -> PResult<Hir> {
        while let Some(c) = self.peek() {
            match c {
                '(' => self.open_group()?,
                ')' => self.close_group()?,
                '|' => {
                    self.pos += 1;
                    let concat = std::mem::take(&mut self.concat);
                    self.alts.push(Hir::concat(concat));
                    self.last = Last::Nothing;
                }
                '*' | '+' | '?' => {
                    let start = self.pos;
                    self.pos += 1;
                    self.check_repeatable(start)?;
                    let (min, max) = match c {
                        '*' => (0, None),
                        '+' => (1, None),
                        _ => (0, Some(1)),
                    };
                    self.apply_repeat(start, min, max)?;
                }
                '{' => self.counted_repeat()?,
                '[' => {
                    let set = self.parse_class()?;
                    self.push_atom(Hir::class(set));
                }
                '.' => {
                    self.pos += 1;
                    let set = if self.flags.s {
                        ClassSet::all()
                    } else {
                        ClassSet::all_except_newline()
                    };
                    self.push_atom(Hir::class(set));
                }
                '^' => {
                    self.pos += 1;
                    let look = if self.flags.m {
                        Look::StartLine
                    } else {
                        Look::StartText
                    };
                    self.push_atom(Hir::look(look));
                }
                '$' => {
                    self.pos += 1;
                    let look = if self.flags.m {
                        Look::EndLine
                    } else {
                        Look::EndText
                    };
                    self.push_atom(Hir::look(look));
                }
                '\\' => match self.parse_escape(false)? {
                    Escape::Char(c) => self.push_literal(c),
                    Escape::Class(set) => self.push_atom(Hir::class(set)),
                    Escape::Look(look) => self.push_atom(Hir::look(look)),
                },
                _ => {
                    self.pos += c.len_utf8();
                    self.push_literal(c);
                }
            }
        }
        if let Some(frame) = self.stack.last() {
            return err(ErrorKind::UnclosedGroup, frame.open, frame.open + 1);
        }
        Ok(self.finish_alternation())
    }

    fn push_atom(&mut self, hir: Hir) {
        self.concat.push(hir);
        self.last = Last::Atom;
    }

    fn push_literal(&mut self, c: char) {
        if self.flags.i {
            self.push_atom(Hir::class(fold::fold_char(c)));
        } else {
            self.push_atom(Hir::char(c));
        }
    }

    fn finish_alternation(&mut self) -> Hir {
        let concat = Hir::concat(std::mem::take(&mut self.concat));
        if self.alts.is_empty() {
            concat
        } else {
            let mut alts = std::mem::take(&mut self.alts);
            alts.push(concat);
            Hir::alt(alts)
        }
    }

    // ---- repetition -------------------------------------------------------

    fn check_repeatable(&self, start: usize) -> PResult<()> {
        match self.last {
            Last::Atom => Ok(()),
            Last::Nothing => err(ErrorKind::RepetitionMissing, start, start + 1),
            Last::Repeated => err(ErrorKind::RepetitionStacked, start, start + 1),
        }
    }

    fn apply_repeat(&mut self, start: usize, min: u32, max: Option<u32>) -> PResult<()> {
        let greedy = match self.peek() {
            Some('?') => {
                self.pos += 1;
                false
            }
            Some('+') => {
                return err(
                    ErrorKind::Unsupported {
                        feature: "possessive repetition",
                    },
                    self.pos,
                    self.pos + 1,
                )
            }
            _ => true,
        };
        let Some(sub) = self.concat.pop() else {
            return err(ErrorKind::RepetitionMissing, start, start + 1);
        };
        let hir = Hir::repeat(min, max, greedy, sub);
        if hir.props.depth > self.nest_limit {
            return err(
                ErrorKind::NestLimitExceeded {
                    limit: self.nest_limit,
                },
                start,
                self.pos,
            );
        }
        self.concat.push(hir);
        self.last = Last::Repeated;
        Ok(())
    }

    fn counted_repeat(&mut self) -> PResult<()> {
        let open = self.pos;
        self.pos += 1;
        self.check_repeatable(open)?;
        let min = self.parse_count(open)?;
        let max = match self.peek() {
            Some('}') => Some(min),
            Some(',') => {
                self.pos += 1;
                if self.peek() == Some('}') {
                    None
                } else {
                    Some(self.parse_count(open)?)
                }
            }
            _ => return err(ErrorKind::RepetitionCountUnclosed, open, self.char_end()),
        };
        if self.peek() != Some('}') {
            return err(ErrorKind::RepetitionCountUnclosed, open, self.char_end());
        }
        self.pos += 1;
        let end = self.pos;
        if min > MAX_REPEAT || max.is_some_and(|m| m > MAX_REPEAT) {
            return err(
                ErrorKind::RepetitionCountTooLarge { limit: MAX_REPEAT },
                open,
                end,
            );
        }
        if let Some(max) = max {
            if max < min {
                return err(ErrorKind::RepetitionCountInvalid { min, max }, open, end);
            }
        }
        self.apply_repeat(open, min, max)
    }

    fn parse_count(&mut self, open: usize) -> PResult<u32> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(d);
            self.pos += 1;
        }
        if self.pos == start {
            return err(ErrorKind::RepetitionCountUnclosed, open, self.char_end());
        }
        Ok(value)
    }

    // ---- groups -----------------------------------------------------------

    fn push_frame(&mut self, open: usize, kind: GroupKind, restore: Flags) -> PResult<()> {
        if self.stack.len() as u64 >= u64::from(self.nest_limit) {
            return err(
                ErrorKind::NestLimitExceeded {
                    limit: self.nest_limit,
                },
                open,
                open + 1,
            );
        }
        self.stack.push(Frame {
            kind,
            open,
            flags: restore,
            alts: std::mem::take(&mut self.alts),
            concat: std::mem::take(&mut self.concat),
        });
        self.last = Last::Nothing;
        Ok(())
    }

    fn next_capture(&mut self, name: Option<&'p str>) -> u32 {
        if let Some(name) = name {
            self.name_set.insert(name);
        }
        self.names.push(name.map(Into::into));
        (self.names.len() - 1) as u32
    }

    fn open_group(&mut self) -> PResult<()> {
        let open = self.pos;
        self.pos += 1;
        if self.peek() != Some('?') {
            let index = self.next_capture(None);
            return self.push_frame(open, GroupKind::Capture(index), self.flags);
        }
        self.pos += 1;
        let unsupported = |feature, end| err(ErrorKind::Unsupported { feature }, open, end);
        match self.peek() {
            None => err(ErrorKind::UnclosedGroup, open, open + 1),
            Some('P') => {
                self.pos += 1;
                match self.peek() {
                    Some('<') => {
                        self.pos += 1;
                        self.named_group(open)
                    }
                    Some('=') => unsupported("backreference", self.pos + 1),
                    Some('>') => unsupported("recursion", self.pos + 1),
                    _ => err(ErrorKind::UnknownFlag { flag: 'P' }, self.pos - 1, self.pos),
                }
            }
            Some('<') => {
                self.pos += 1;
                match self.peek() {
                    Some('=') | Some('!') => unsupported("look-around", self.pos + 1),
                    _ => self.named_group(open),
                }
            }
            Some('=') | Some('!') => unsupported("look-around", self.pos + 1),
            Some('>') => unsupported("atomic group", self.pos + 1),
            Some('#') => unsupported("comment group", self.pos + 1),
            Some(_) => self.flag_group(open),
        }
    }

    fn named_group(&mut self, open: usize) -> PResult<()> {
        let start = self.pos;
        let pat: &'p str = self.pat;
        let Some(rel) = pat[start..].find('>') else {
            return err(ErrorKind::InvalidGroupName, start, pat.len());
        };
        let end = start + rel;
        let name = &pat[start..end];
        let mut chars = name.chars();
        let valid = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return err(ErrorKind::InvalidGroupName, start, end);
        }
        if self.name_set.contains(name) {
            return err(
                ErrorKind::DuplicateGroupName {
                    name: name.to_owned(),
                },
                start,
                end,
            );
        }
        self.pos = end + 1;
        let index = self.next_capture(Some(name));
        self.push_frame(open, GroupKind::Capture(index), self.flags)
    }

    fn flag_group(&mut self, open: usize) -> PResult<()> {
        let mut new = self.flags;
        let mut negated = false;
        let mut dangling = false;
        let mut any = false;
        let mut seen = [false; 3];
        loop {
            let at = self.pos;
            match self.peek() {
                None => return err(ErrorKind::UnclosedGroup, open, open + 1),
                Some(':') => {
                    if dangling {
                        return err(ErrorKind::InvalidFlags, open, at + 1);
                    }
                    self.pos += 1;
                    let restore = self.flags;
                    self.push_frame(open, GroupKind::NonCapture, restore)?;
                    self.flags = new;
                    return Ok(());
                }
                Some(')') => {
                    if !any || dangling {
                        return err(ErrorKind::InvalidFlags, open, at + 1);
                    }
                    self.pos += 1;
                    self.flags = new;
                    self.last = Last::Nothing;
                    return Ok(());
                }
                Some('-') => {
                    if negated {
                        return err(ErrorKind::InvalidFlags, at, at + 1);
                    }
                    negated = true;
                    dangling = true;
                    self.pos += 1;
                }
                Some(f @ ('i' | 'm' | 's')) => {
                    let slot = match f {
                        'i' => 0,
                        'm' => 1,
                        _ => 2,
                    };
                    if seen[slot] {
                        return err(ErrorKind::InvalidFlags, at, at + 1);
                    }
                    seen[slot] = true;
                    let value = !negated;
                    match slot {
                        0 => new.i = value,
                        1 => new.m = value,
                        _ => new.s = value,
                    }
                    any = true;
                    dangling = false;
                    self.pos += 1;
                }
                Some(f @ ('x' | 'U' | 'u' | 'R')) => {
                    let feature = match f {
                        'x' => "verbose flag (?x)",
                        'U' => "swap-greed flag (?U)",
                        'u' => "Unicode flag (?u)",
                        _ => "CRLF flag (?R)",
                    };
                    return err(ErrorKind::Unsupported { feature }, at, at + 1);
                }
                Some(flag) => {
                    return err(ErrorKind::UnknownFlag { flag }, at, at + flag.len_utf8())
                }
            }
        }
    }

    fn close_group(&mut self) -> PResult<()> {
        let at = self.pos;
        self.pos += 1;
        let Some(frame) = self.stack.pop() else {
            return err(ErrorKind::UnopenedGroup, at, at + 1);
        };
        let inner = self.finish_alternation();
        self.flags = frame.flags;
        self.alts = frame.alts;
        self.concat = frame.concat;
        let hir = match frame.kind {
            GroupKind::Capture(index) => Hir::capture(index, inner),
            GroupKind::NonCapture => inner,
        };
        if hir.props.depth > self.nest_limit {
            return err(
                ErrorKind::NestLimitExceeded {
                    limit: self.nest_limit,
                },
                frame.open,
                self.pos,
            );
        }
        self.push_atom(hir);
        Ok(())
    }

    // ---- escapes ----------------------------------------------------------

    fn parse_escape(&mut self, in_class: bool) -> PResult<Escape> {
        let start = self.pos;
        self.pos += 1;
        let Some(c) = self.peek() else {
            return err(ErrorKind::TrailingBackslash, start, start + 1);
        };
        let after = self.pos + c.len_utf8();
        let unsupported = |feature| err(ErrorKind::Unsupported { feature }, start, after);
        let escape = match c {
            'd' => Escape::Class(ClassSet::perl(Perl::Digit, false)),
            'D' => Escape::Class(ClassSet::perl(Perl::Digit, true)),
            'w' => Escape::Class(ClassSet::perl(Perl::Word, false)),
            'W' => Escape::Class(ClassSet::perl(Perl::Word, true)),
            's' => Escape::Class(ClassSet::perl(Perl::Space, false)),
            'S' => Escape::Class(ClassSet::perl(Perl::Space, true)),
            'n' => Escape::Char('\n'),
            't' => Escape::Char('\t'),
            'r' => Escape::Char('\r'),
            'f' => Escape::Char('\x0C'),
            'v' => Escape::Char('\x0B'),
            'a' => Escape::Char('\x07'),
            'e' => Escape::Char('\x1B'),
            'x' | 'u' => return self.parse_hex(start, c),
            'b' if !in_class => Escape::Look(Look::WordBoundary),
            'B' if !in_class => Escape::Look(Look::NotWordBoundary),
            'A' if !in_class => Escape::Look(Look::StartText),
            'z' if !in_class => Escape::Look(Look::EndText),
            '1'..='9' => return unsupported("backreference"),
            '0' => return unsupported("octal escape (use \\x00)"),
            'p' | 'P' => return unsupported("Unicode property class"),
            'Z' => return unsupported("\\Z (use \\z)"),
            'G' => return unsupported("\\G anchor"),
            'K' => return unsupported("\\K match reset"),
            '<' | '>' => return unsupported("word-start/word-end assertion"),
            c if c.is_ascii_punctuation() => Escape::Char(c),
            c => return err(ErrorKind::InvalidEscape { c }, start, after),
        };
        self.pos = after;
        Ok(escape)
    }

    fn parse_hex(&mut self, start: usize, kind: char) -> PResult<Escape> {
        self.pos += 1;
        let digits_start;
        let digits_end;
        if self.peek() == Some('{') {
            self.pos += 1;
            digits_start = self.pos;
            loop {
                match self.peek() {
                    None => return err(ErrorKind::InvalidHexEscape, start, self.pos),
                    Some('}') => break,
                    Some(c) if c.is_ascii_hexdigit() => self.pos += 1,
                    Some(_) => return err(ErrorKind::InvalidHexEscape, start, self.char_end()),
                }
            }
            digits_end = self.pos;
            self.pos += 1;
            let n = digits_end - digits_start;
            if n == 0 || n > 6 {
                return err(ErrorKind::InvalidHexEscape, start, self.pos);
            }
        } else {
            let n = if kind == 'x' { 2 } else { 4 };
            digits_start = self.pos;
            for _ in 0..n {
                match self.peek() {
                    Some(c) if c.is_ascii_hexdigit() => self.pos += 1,
                    _ => return err(ErrorKind::InvalidHexEscape, start, self.char_end()),
                }
            }
            digits_end = self.pos;
        }
        let value =
            u32::from_str_radix(&self.pat[digits_start..digits_end], 16).unwrap_or(u32::MAX);
        match char::from_u32(value) {
            Some(c) => Ok(Escape::Char(c)),
            None => err(ErrorKind::InvalidCodepoint { value }, start, self.pos),
        }
    }

    // ---- classes ----------------------------------------------------------

    fn class_char(&mut self) -> PResult<Result<char, ClassSet>> {
        let Some(c) = self.peek() else {
            return err(ErrorKind::UnclosedClass, self.pos, self.pos);
        };
        if c != '\\' {
            self.pos += c.len_utf8();
            return Ok(Ok(c));
        }
        let start = self.pos;
        match self.parse_escape(true)? {
            Escape::Char(c) => Ok(Ok(c)),
            Escape::Class(set) => Ok(Err(set)),
            Escape::Look(_) => err(ErrorKind::InvalidEscape { c: 'b' }, start, self.pos),
        }
    }

    fn parse_class(&mut self) -> PResult<ClassSet> {
        let open = self.pos;
        self.pos += 1;
        let negated = if self.peek() == Some('^') {
            self.pos += 1;
            true
        } else {
            false
        };
        let set_op = |at: usize| {
            err(
                ErrorKind::Unsupported {
                    feature: "character class set operation",
                },
                at,
                at + 2,
            )
        };
        let mut lits = ClassSet::new();
        let mut perl = ClassSet::new();
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return err(ErrorKind::UnclosedClass, open, open + 1);
            };
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            if c == '[' {
                return err(
                    ErrorKind::Unsupported {
                        feature: "nested or POSIX character class",
                    },
                    self.pos,
                    self.pos + 1,
                );
            }
            if matches!(c, '&' | '~' | '-') && self.peek2() == Some(c) {
                return set_op(self.pos);
            }
            first = false;
            let item_start = self.pos;
            match self.class_char()? {
                Err(set) => perl.union(&set),
                Ok(lo) => {
                    let is_range =
                        self.peek() == Some('-') && self.peek2().is_some_and(|n| n != ']');
                    if !is_range {
                        lits.push(lo, lo);
                        continue;
                    }
                    if self.peek2() == Some('-') {
                        return set_op(self.pos);
                    }
                    self.pos += 1;
                    match self.class_char()? {
                        Ok(hi) if hi >= lo => lits.push(lo, hi),
                        Ok(hi) => {
                            return err(
                                ErrorKind::InvalidClassRange { start: lo, end: hi },
                                item_start,
                                self.pos,
                            )
                        }
                        Err(_) => {
                            return err(
                                ErrorKind::Unsupported {
                                    feature: "class range with a class endpoint",
                                },
                                item_start,
                                self.pos,
                            )
                        }
                    }
                }
            }
        }
        lits.canonicalize();
        if self.flags.i {
            lits = fold::fold_class(&lits);
        }
        lits.union(&perl);
        if negated {
            lits.negate();
        }
        Ok(lits)
    }
}
