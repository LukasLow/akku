//! Glob matching and filesystem expansion.
//!
//! [`glob`] expands a pattern into matching paths; [`GlobMatcher`] tests a
//! single path against a pattern. The syntax covers `*`, `?`, whole-segment
//! `**`, character classes (`[a-z]`, `[!a-z]`), brace alternation (`{a,b}`)
//! and backslash escapes.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// One parsed wildcard piece inside a single path segment.
#[derive(Debug, Clone)]
enum Piece {
    /// A literal character (`\x` escapes included).
    Char(char),
    /// `*`, any run of characters within one segment.
    Any,
    /// `?`, exactly one character.
    One,
    /// A character class such as `[a-z]` or `[!0-9]`.
    Class {
        /// `true` for `[!...]`.
        negated: bool,
        /// The class members between the brackets.
        items: Vec<ClassItem>,
    },
    /// A `{a,b}` alternation; each alternative is its own piece run.
    Alternatives(Vec<Vec<Piece>>),
}

/// A member of a character class.
#[derive(Debug, Clone, Copy)]
enum ClassItem {
    /// A single character.
    Char(char),
    /// An inclusive range `lo..=hi`.
    Range(char, char),
}

/// A parsed pattern segment (the text between two path separators).
#[derive(Debug, Clone)]
enum Seg {
    /// A segment without wildcards, compared byte-for-byte.
    Literal(String),
    /// A segment with wildcards.
    Pattern(Vec<Piece>),
    /// `**`, matching zero or more whole segments.
    Any,
}

/// Parse one whole pattern segment; `**` must be the only content.
fn parse_segment(seg: &[(usize, char)]) -> Result<Seg, GlobError> {
    if seg.len() == 2 && seg[0].1 == '*' && seg[1].1 == '*' {
        return Ok(Seg::Any);
    }
    let pieces = parse_pieces(seg)?;
    if pieces.iter().all(|piece| matches!(piece, Piece::Char(_))) {
        let literal: String = pieces
            .into_iter()
            .map(|piece| match piece {
                Piece::Char(c) => c,
                _ => unreachable!("only Char pieces survive the all check"),
            })
            .collect();
        return Ok(Seg::Literal(literal));
    }
    Ok(Seg::Pattern(pieces))
}

/// Parse the wildcard pieces of one segment.
fn parse_pieces(seg: &[(usize, char)]) -> Result<Vec<Piece>, GlobError> {
    let mut pieces = Vec::new();
    let mut i = 0;
    while i < seg.len() {
        let (pos, c) = seg[i];
        match c {
            '*' => {
                // `**` inside a non-whole segment is always a syntax error.
                if i + 1 < seg.len() && seg[i + 1].1 == '*' {
                    return Err(syntax(pos, PatternIssue::DoubleStarNotWholeSegment));
                }
                pieces.push(Piece::Any);
                i += 1;
            }
            '?' => {
                pieces.push(Piece::One);
                i += 1;
            }
            '\\' => {
                let Some((_, escaped)) = seg.get(i + 1) else {
                    return Err(syntax(pos, PatternIssue::UnexpectedEscape));
                };
                pieces.push(Piece::Char(*escaped));
                i += 2;
            }
            '[' => {
                let (next, class) = parse_class(seg, i)?;
                pieces.push(class);
                i = next;
            }
            '{' => {
                let (next, brace) = parse_brace(seg, i)?;
                pieces.push(brace);
                i = next;
            }
            _ => {
                pieces.push(Piece::Char(c));
                i += 1;
            }
        }
    }
    Ok(pieces)
}

/// Parse a `[...]` class starting at segment position `i`.
fn parse_class(seg: &[(usize, char)], mut i: usize) -> Result<(usize, Piece), GlobError> {
    let open_pos = seg[i].0;
    i += 1;
    let negated = i < seg.len() && seg[i].1 == '!';
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    // A `]` in first position is a literal member, not the closer.
    let mut at_start = true;
    loop {
        let Some(&(pos, c)) = seg.get(i) else {
            return Err(syntax(open_pos, PatternIssue::UnclosedClass));
        };
        if c == ']' && !at_start {
            return Ok((i + 1, Piece::Class { negated, items }));
        }
        at_start = false;
        if i + 2 < seg.len() && seg[i + 1].1 == '-' && seg[i + 2].1 != ']' {
            let hi = seg[i + 2].1;
            if c > hi {
                return Err(syntax(pos, PatternIssue::InvalidClassRange));
            }
            items.push(ClassItem::Range(c, hi));
            i += 3;
        } else {
            items.push(ClassItem::Char(c));
            i += 1;
        }
    }
}

/// Parse a `{...}` alternation starting at segment position `i`.
fn parse_brace(seg: &[(usize, char)], mut i: usize) -> Result<(usize, Piece), GlobError> {
    let open_pos = seg[i].0;
    i += 1;
    let mut alternatives = vec![Vec::new()];
    loop {
        let Some(&(pos, c)) = seg.get(i) else {
            return Err(syntax(open_pos, PatternIssue::UnclosedBrace));
        };
        match c {
            '{' => return Err(syntax(pos, PatternIssue::NestedBrace)),
            '}' => {
                if alternatives.len() == 1 && alternatives[0].is_empty() {
                    return Err(syntax(open_pos, PatternIssue::EmptyBrace));
                }
                return Ok((i + 1, Piece::Alternatives(alternatives)));
            }
            ',' => {
                alternatives.push(Vec::new());
                i += 1;
            }
            '[' => {
                let (next, class) = parse_class(seg, i)?;
                if let Some(last) = alternatives.last_mut() {
                    last.push(class);
                }
                i = next;
            }
            '*' => {
                if i + 1 < seg.len() && seg[i + 1].1 == '*' {
                    return Err(syntax(pos, PatternIssue::DoubleStarNotWholeSegment));
                }
                if let Some(last) = alternatives.last_mut() {
                    last.push(Piece::Any);
                }
                i += 1;
            }
            '?' => {
                if let Some(last) = alternatives.last_mut() {
                    last.push(Piece::One);
                }
                i += 1;
            }
            '\\' => {
                let Some((_, escaped)) = seg.get(i + 1) else {
                    return Err(syntax(pos, PatternIssue::UnexpectedEscape));
                };
                if let Some(last) = alternatives.last_mut() {
                    last.push(Piece::Char(*escaped));
                }
                i += 2;
            }
            _ => {
                if let Some(last) = alternatives.last_mut() {
                    last.push(Piece::Char(c));
                }
                i += 1;
            }
        }
    }
}

/// Compile a pattern into segments, reporting whether it is absolute.
fn compile(pattern: &str) -> Result<(Vec<Seg>, bool), GlobError> {
    let absolute = pattern.starts_with('/');
    let chars: Vec<(usize, char)> = pattern.char_indices().collect();
    let mut pos = usize::from(absolute);
    let mut segments = Vec::new();
    loop {
        let start = pos;
        while pos < chars.len() && chars[pos].1 != '/' {
            pos += 1;
        }
        let slice = &chars[start..pos];
        if !slice.is_empty() {
            segments.push(parse_segment(slice)?);
        }
        if pos >= chars.len() {
            break;
        }
        pos += 1; // skip the separator
    }
    Ok((segments, absolute))
}

/// Compile a filesystem pattern into its literal root and segments.
#[cfg(not(windows))]
fn compile_for_fs(pattern: &str) -> Result<(PathBuf, Vec<Seg>), GlobError> {
    let (segments, absolute) = compile(pattern)?;
    let root = if absolute { "/" } else { "" };
    Ok((PathBuf::from(root), segments))
}

/// Compile a filesystem pattern into its literal root and segments.
///
/// Windows paths carry a prefix (`C:`, `\\server\share`, `\\?\C:`) that
/// is not a glob segment, and use `\` as a separator, so the prefix and root
/// are split off with [`std::path::Path`] semantics and the rest is compiled
/// with `\` read as `/`.
#[cfg(windows)]
fn compile_for_fs(pattern: &str) -> Result<(PathBuf, Vec<Seg>), GlobError> {
    use std::path::Component;

    let prefix_len = match Path::new(pattern).components().next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str().len(),
        _ => 0,
    };
    // `prefix_len` is the byte length of a leading `str` slice, so it is a
    // char boundary.
    let (prefix, rest) = pattern.split_at(prefix_len);
    let rest = rest.replace('\\', "/");
    let (segments, absolute) = compile(&rest).map_err(|err| match err {
        GlobError::Syntax { position, issue } => GlobError::Syntax {
            position: position + prefix_len,
            issue,
        },
        other => other,
    })?;
    let mut root = String::from(prefix);
    if absolute {
        root.push('\\');
    }
    Ok((PathBuf::from(root), segments))
}

/// The issue found while parsing a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternIssue {
    /// A `\` with no character behind it.
    UnexpectedEscape,
    /// A `[` without a matching `]`.
    UnclosedClass,
    /// A `{` without a matching `}`.
    UnclosedBrace,
    /// A `{` inside another `{...}`.
    NestedBrace,
    /// Curly braces with no content at all (`{}`).
    EmptyBrace,
    /// A `**` that is not a complete path segment on its own.
    DoubleStarNotWholeSegment,
    /// A class range whose start is greater than its end, as in `[z-a]`.
    InvalidClassRange,
}

impl fmt::Display for PatternIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            PatternIssue::UnexpectedEscape => "unterminated escape character",
            PatternIssue::UnclosedClass => "unterminated character class",
            PatternIssue::UnclosedBrace => "unterminated brace expression",
            PatternIssue::NestedBrace => "nested braces are not supported",
            PatternIssue::EmptyBrace => "empty brace expression",
            PatternIssue::DoubleStarNotWholeSegment => "`**` must appear as a whole path segment",
            PatternIssue::InvalidClassRange => {
                "invalid character class range (start is greater than end)"
            }
        };
        f.write_str(text)
    }
}

/// An error from glob matching or expansion.
#[derive(Debug)]
pub enum GlobError {
    /// The pattern is syntactically invalid.
    Syntax {
        /// Byte position in the pattern where the issue starts.
        position: usize,
        /// The kind of problem found.
        issue: PatternIssue,
    },
    /// The file system could not be read while expanding the pattern.
    Io(io::Error),
}

impl GlobError {
    fn syntax_error(position: usize, issue: PatternIssue) -> Self {
        GlobError::Syntax { position, issue }
    }
}

impl fmt::Display for GlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GlobError::Syntax { position, issue } => {
                write!(f, "invalid glob pattern at position {position}: {issue}")
            }
            GlobError::Io(err) => write!(f, "glob expansion failed: {err}"),
        }
    }
}

impl Error for GlobError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            GlobError::Io(err) => Some(err),
            GlobError::Syntax { .. } => None,
        }
    }
}

fn syntax(position: usize, issue: PatternIssue) -> GlobError {
    GlobError::syntax_error(position, issue)
}

/// A matcher for glob patterns.
///
/// The pattern is compiled once; [`is_match`](GlobMatcher::is_match) then
/// checks paths against it. Non-UTF-8 paths and paths with empty segments
/// never match. Hidden dot names are matched like any other name.
///
/// # Examples
///
/// ```
/// use akku::fs::GlobMatcher;
///
/// let matcher = GlobMatcher::new("logs/**/*.log").unwrap();
/// assert!(matcher.is_match("logs/2026/app.log"));
/// assert!(!matcher.is_match("app.log"));
/// ```
#[derive(Debug, Clone)]
pub struct GlobMatcher {
    pattern: String,
    absolute: bool,
    segments: Vec<Seg>,
}

impl GlobMatcher {
    /// Compile `pattern` into a matcher.
    ///
    /// # Errors
    ///
    /// Returns [`GlobError::Syntax`] if the pattern is invalid.
    pub fn new(pattern: &str) -> Result<Self, GlobError> {
        let (segments, absolute) = compile(pattern)?;
        Ok(Self {
            pattern: pattern.to_owned(),
            absolute,
            segments,
        })
    }

    /// The pattern this matcher was built from.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Whether `path` matches the pattern.
    pub fn is_match(&self, path: &str) -> bool {
        let mut parts: Vec<&str> = path.split('/').collect();
        if self.absolute {
            if !path.starts_with('/') {
                return false;
            }
            if parts.first() == Some(&"") {
                parts.remove(0);
            }
        }
        match_segments(&self.segments, &parts)
    }
}

impl FromStr for GlobMatcher {
    type Err = GlobError;

    fn from_str(pattern: &str) -> Result<Self, Self::Err> {
        GlobMatcher::new(pattern)
    }
}

/// Match pattern segments against path parts, honoring `**`.
fn match_segments(patterns: &[Seg], parts: &[&str]) -> bool {
    match patterns.split_first() {
        None => parts.is_empty(),
        Some((first, rest)) => match first {
            Seg::Any => {
                match_segments(rest, parts)
                    || (!parts.is_empty() && match_segments(patterns, &parts[1..]))
            }
            Seg::Literal(literal) => {
                !parts.is_empty()
                    && parts[0] == literal.as_str()
                    && match_segments(rest, &parts[1..])
            }
            Seg::Pattern(pieces) => {
                !parts.is_empty()
                    && {
                        let text: Vec<char> = parts[0].chars().collect();
                        match_pieces(pieces, &text, 0, 0)
                    }
                    && match_segments(rest, &parts[1..])
            }
        },
    }
}

/// Backtracking matcher for the wildcard pieces of one segment.
fn match_pieces(pieces: &[Piece], text: &[char], ti: usize, pi: usize) -> bool {
    if pi == pieces.len() {
        return ti == text.len();
    }
    match &pieces[pi] {
        Piece::Char(c) => {
            ti < text.len() && text[ti] == *c && match_pieces(pieces, text, ti + 1, pi + 1)
        }
        Piece::One => ti < text.len() && match_pieces(pieces, text, ti + 1, pi + 1),
        Piece::Any => (ti..=text.len()).any(|k| match_pieces(pieces, text, k, pi + 1)),
        Piece::Class { negated, items } => {
            if ti >= text.len() {
                return false;
            }
            let c = text[ti];
            let inside = items.iter().any(|item| match item {
                ClassItem::Char(member) => *member == c,
                ClassItem::Range(lo, hi) => (*lo..=*hi).contains(&c),
            });
            inside != *negated && match_pieces(pieces, text, ti + 1, pi + 1)
        }
        Piece::Alternatives(alternatives) => alternatives.iter().any(|alt| {
            if alt.is_empty() {
                match_pieces(pieces, text, ti, pi + 1)
            } else {
                let mut combined = Vec::with_capacity(alt.len() + pieces.len() - pi - 1);
                combined.extend_from_slice(alt);
                combined.extend_from_slice(&pieces[pi + 1..]);
                match_pieces(&combined, text, ti, 0)
            }
        }),
    }
}

/// A child found while expanding a glob.
struct Kid {
    name: std::ffi::OsString,
    is_dir: bool,
}

/// List the children of `base`, treating missing and non-directory paths as
/// empty.
fn read_kids(base: &Path) -> Result<Vec<Kid>, GlobError> {
    let dir: &Path = if base.as_os_str().is_empty() {
        Path::new(".")
    } else {
        base
    };
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(err)
            if err.kind() == io::ErrorKind::NotFound
                || err.kind() == io::ErrorKind::NotADirectory =>
        {
            return Ok(Vec::new());
        }
        Err(err) => return Err(GlobError::Io(err)),
    };
    let mut kids = Vec::new();
    for entry in read {
        let entry = entry.map_err(GlobError::Io)?;
        let name = entry.file_name();
        let is_dir = entry.file_type().map_err(GlobError::Io)?.is_dir();
        kids.push(Kid { name, is_dir });
    }
    Ok(kids)
}

/// Recursively expand one pattern segment position.
fn expand(base: &Path, segments: &[Seg], out: &mut Vec<PathBuf>) -> Result<(), GlobError> {
    let Some((first, rest)) = segments.split_first() else {
        if base.as_os_str().is_empty() {
            out.push(PathBuf::from("."));
        } else {
            out.push(base.to_path_buf());
        }
        return Ok(());
    };
    match first {
        Seg::Literal(literal) => {
            let child = base.join(literal);
            if rest.is_empty() {
                match fs::symlink_metadata(&child) {
                    Ok(_) => out.push(child),
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(GlobError::Io(err)),
                }
            } else {
                // The read below silently handles missing directories.
                expand(&child, rest, out)?;
            }
        }
        Seg::Any => {
            // `**` matches zero segments first, then one or more.
            expand(base, rest, out)?;
            for kid in read_kids(base)? {
                let child = base.join(&kid.name);
                if kid.is_dir {
                    expand(&child, segments, out)?;
                } else if rest.is_empty() {
                    out.push(child);
                }
            }
        }
        Seg::Pattern(pieces) => {
            for kid in read_kids(base)? {
                let Some(name) = kid.name.to_str() else {
                    continue;
                };
                let text: Vec<char> = name.chars().collect();
                if !match_pieces(pieces, &text, 0, 0) {
                    continue;
                }
                let child = base.join(&kid.name);
                if rest.is_empty() {
                    out.push(child);
                } else if kid.is_dir {
                    expand(&child, rest, out)?;
                }
            }
        }
    }
    Ok(())
}

/// Expand `pattern` into the matching paths.
///
/// The result is sorted for deterministic order and deduplicated. Missing
/// directories along the pattern contribute no matches and never fail;
/// unreadable directories surface as [`GlobError::Io`]. Symlinked
/// directories are listed but never followed during expansion.
///
/// # Examples
///
/// ```
/// use akku::fs::glob;
///
/// let root = std::env::temp_dir().join("akku-glob-doctest");
/// let _ = std::fs::remove_dir_all(&root);
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(root.join("a.rs"), b"").unwrap();
/// std::fs::write(root.join("b.txt"), b"").unwrap();
///
/// let pattern = format!("{}/*.rs", root.display());
/// let found = glob(&pattern).unwrap();
/// assert_eq!(found, vec![root.join("a.rs")]);
/// let _ = std::fs::remove_dir_all(&root);
/// ```
///
/// # Platform notes
///
/// On Windows a leading path prefix (`C:`, `\\server\share`, `\\?\C:`)
/// and root are taken literally, and `\` is a path separator like `/`, so
/// `C:\logs\*.txt` works; escape wildcards with a class instead (`[*]`,
/// `[?]`). Elsewhere `\` escapes the next character.
///
/// # Errors
///
/// Returns [`GlobError::Syntax`] for bad patterns and [`GlobError::Io`]
/// when unreadable directories abort the expansion.
pub fn glob(pattern: &str) -> Result<Vec<PathBuf>, GlobError> {
    let (root, segments) = compile_for_fs(pattern)?;
    let mut out = Vec::new();
    expand(&root, &segments, &mut out)?;
    out.sort();
    out.dedup();
    Ok(out)
}
