//! Splitting template source and tokenizing tag contents.

use super::error::{ErrorKind, Fail, RawError};

/// A token inside `{{ }}` or `{% %}`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    /// Punctuation and operators.
    Sym(&'static str),
    /// The closing delimiter (`}}` or `%}`); its offset is where it starts.
    End(&'static str),
}

/// A token with the byte offset where it starts.
#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) tok: Tok,
    pub(crate) offset: usize,
}

impl Tok {
    /// A short description for error messages.
    pub(crate) fn describe(&self) -> String {
        match self {
            Tok::Ident(name) => format!("`{name}`"),
            Tok::Int(_) | Tok::Float(_) => "a number".to_owned(),
            Tok::Str(_) => "a string".to_owned(),
            Tok::Sym(symbol) => format!("`{symbol}`"),
            Tok::End(symbol) => format!("`{symbol}`"),
        }
    }
}

/// The kind of a delimited segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SegmentKind {
    Var,
    Tag,
    Comment,
}

/// A `{{ }}`, `{% %}` or `{# #}` segment of the source.
#[derive(Debug)]
pub(crate) struct Segment {
    pub(crate) kind: SegmentKind,
    /// Offset of the opening delimiter.
    pub(crate) open: usize,
    /// Offset of the first byte of the content (after `{{` and a `-`).
    pub(crate) content_start: usize,
    /// Offset just after the content (before a trailing `-` and the closer).
    pub(crate) content_end: usize,
    /// Offset just after the closing delimiter.
    pub(crate) end: usize,
    /// `{{-`: trim whitespace before.
    pub(crate) trim_before: bool,
    /// `-}}`: trim whitespace after.
    pub(crate) trim_after: bool,
}

/// Find the next segment at or after `from`.
///
/// Returns `Ok(None)` when no delimiter follows.
pub(crate) fn next_segment(source: &str, from: usize) -> Result<Option<Segment>, RawError> {
    let bytes = source.as_bytes();
    let mut i = from;
    let open = loop {
        match bytes[i..].iter().position(|&b| b == b'{') {
            None => return Ok(None),
            Some(offset) => {
                let at = i + offset;
                if matches!(bytes.get(at + 1), Some(b'{' | b'%' | b'#')) {
                    break at;
                }
                i = at + 1;
            }
        }
    };
    let (kind, open_text, closer) = match bytes[open + 1] {
        b'{' => (SegmentKind::Var, "{{", "}}"),
        b'%' => (SegmentKind::Tag, "{%", "%}"),
        _ => (SegmentKind::Comment, "{#", "#}"),
    };
    let trim_before = bytes.get(open + 2) == Some(&b'-');
    let content_start = open + 2 + usize::from(trim_before);
    let closer_bytes = closer.as_bytes();
    let unclosed = || Fail::new(ErrorKind::UnclosedTag { open: open_text }, open);
    let mut j = content_start;
    let close_at = loop {
        match bytes.get(j) {
            None => return Err(unclosed()),
            Some(&quote @ (b'"' | b'\'')) if kind != SegmentKind::Comment => {
                // Skip a string literal so a `}}` inside it does not close.
                let mut k = j + 1;
                loop {
                    match bytes.get(k) {
                        None | Some(b'\n') => {
                            return Err(Fail::new(ErrorKind::UnterminatedString, j))
                        }
                        Some(b'\\') => k += 2,
                        Some(&b) if b == quote => break,
                        Some(_) => k += 1,
                    }
                }
                j = k + 1;
            }
            Some(&b) if b == closer_bytes[0] && bytes.get(j + 1) == Some(&closer_bytes[1]) => {
                break j
            }
            Some(_) => j += 1,
        }
    };
    let trim_after = close_at > content_start && bytes[close_at - 1] == b'-';
    let content_end = close_at - usize::from(trim_after);
    Ok(Some(Segment {
        kind,
        open,
        content_start,
        content_end,
        end: close_at + 2,
        trim_before,
        trim_after,
    }))
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Tokenize `source[start..end]`, finishing with an `End` token at `end_at`.
pub(crate) fn tokenize(
    source: &str,
    start: usize,
    end: usize,
    end_at: usize,
    closer: &'static str,
) -> Result<Vec<Token>, RawError> {
    let mut tokens = Vec::new();
    let mut chars = source[start..end].char_indices().peekable();
    while let Some((relative, c)) = chars.next() {
        let offset = start + relative;
        if c.is_whitespace() {
            continue;
        }
        let push = |tokens: &mut Vec<Token>, tok: Tok| tokens.push(Token { tok, offset });
        match c {
            _ if is_ident_start(c) => {
                let mut stop = relative + c.len_utf8();
                while let Some(&(next, ch)) = chars.peek() {
                    if is_ident_char(ch) {
                        stop = next + ch.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                push(
                    &mut tokens,
                    Tok::Ident(source[offset..start + stop].to_owned()),
                );
            }
            '0'..='9' => {
                let mut stop = relative + 1;
                let mut is_float = false;
                while let Some(&(next, ch)) = chars.peek() {
                    if ch.is_ascii_digit() {
                        stop = next + 1;
                        chars.next();
                    } else if ch == '.' && !is_float {
                        // `1.5` is a float; `1.` followed by a non-digit is not.
                        let after = source[start + next + 1..end].chars().next();
                        if after.is_some_and(|a| a.is_ascii_digit()) {
                            is_float = true;
                            stop = next + 1;
                            chars.next();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let text = &source[offset..start + stop];
                let tok = if is_float {
                    text.parse::<f64>().ok().map(Tok::Float)
                } else {
                    text.parse::<i64>().ok().map(Tok::Int)
                };
                match tok {
                    Some(tok) => push(&mut tokens, tok),
                    None => return Err(Fail::new(ErrorKind::InvalidNumber, offset)),
                }
            }
            '"' | '\'' => {
                let mut text = String::new();
                loop {
                    match chars.next() {
                        None => return Err(Fail::new(ErrorKind::UnterminatedString, offset)),
                        Some((_, ch)) if ch == c => break,
                        Some((_, '\\')) => match chars.next() {
                            Some((_, 'n')) => text.push('\n'),
                            Some((_, 't')) => text.push('\t'),
                            Some((_, 'r')) => text.push('\r'),
                            Some((_, other)) => text.push(other),
                            None => return Err(Fail::new(ErrorKind::UnterminatedString, offset)),
                        },
                        Some((_, ch)) => text.push(ch),
                    }
                }
                push(&mut tokens, Tok::Str(text));
            }
            '=' | '!' | '<' | '>' => {
                let double = chars.peek().is_some_and(|&(_, next)| next == '=');
                let symbol = match (c, double) {
                    ('=', true) => "==",
                    ('!', true) => "!=",
                    ('<', true) => "<=",
                    ('>', true) => ">=",
                    ('<', false) => "<",
                    ('>', false) => ">",
                    ('=', false) => "=",
                    _ => {
                        return Err(Fail::new(
                            ErrorKind::UnexpectedToken {
                                expected: "a valid token",
                                found: "`!`".to_owned(),
                            },
                            offset,
                        ))
                    }
                };
                if double {
                    chars.next();
                }
                push(&mut tokens, Tok::Sym(symbol));
            }
            '.' | ',' | '|' | '(' | ')' | '[' | ']' | '+' | '-' | '*' | '/' | '%' | '~' => {
                let symbol = match c {
                    '.' => ".",
                    ',' => ",",
                    '|' => "|",
                    '(' => "(",
                    ')' => ")",
                    '[' => "[",
                    ']' => "]",
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '%' => "%",
                    _ => "~",
                };
                push(&mut tokens, Tok::Sym(symbol));
            }
            other => {
                return Err(Fail::new(
                    ErrorKind::UnexpectedToken {
                        expected: "a valid token",
                        found: format!("`{other}`"),
                    },
                    offset,
                ))
            }
        }
    }
    tokens.push(Token {
        tok: Tok::End(closer),
        offset: end_at,
    });
    Ok(tokens)
}
