//! The lexical layer: characters, positions, line breaks and indentation.
//!
//! The scanner knows nothing about YAML grammar. It tracks a byte offset plus
//! one-based line and character-column numbers, treats `\n`, `\r\n` and a lone
//! `\r` as a single break, and skips a leading byte order mark.

use super::error::Position;

/// A cursor over the input that tracks line and column.
///
/// The scanner is `Copy`, so the parser can snapshot it and backtrack without
/// allocating.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scanner<'a> {
    input: &'a str,
    pos: usize,
    line: u32,
    column: u32,
}

impl<'a> Scanner<'a> {
    /// Create a scanner at the start of the input, skipping a leading BOM.
    pub(crate) fn new(input: &'a str) -> Self {
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        Self {
            input,
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    /// The character at the cursor.
    pub(crate) fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    /// The character before the cursor.
    pub(crate) fn peek_prev(&self) -> Option<char> {
        self.input[..self.pos].chars().next_back()
    }

    /// Whether the remaining input starts with `text`.
    pub(crate) fn starts_with(&self, text: &str) -> bool {
        self.input[self.pos..].starts_with(text)
    }

    /// Whether the cursor is at the end of the input.
    pub(crate) fn at_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    /// The one-based character column of the cursor.
    pub(crate) fn column(&self) -> u32 {
        self.column
    }

    /// The position of the cursor.
    pub(crate) fn position(&self) -> Position {
        Position {
            offset: self.pos,
            line: self.line,
            column: self.column,
        }
    }

    /// Whether the cursor sits on the first character of a line.
    pub(crate) fn at_line_start(&self) -> bool {
        self.column == 1
    }

    /// Advance by one character, counting a break as a single line change.
    ///
    /// `\r\n` is consumed as one break and reported as `'\r'`.
    pub(crate) fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        match ch {
            '\n' => {
                self.line += 1;
                self.column = 1;
            }
            '\r' => {
                if self.input[self.pos..].starts_with('\n') {
                    self.pos += 1;
                }
                self.line += 1;
                self.column = 1;
            }
            _ => self.column += 1,
        }
        Some(ch)
    }

    /// Skip spaces and tabs, but no breaks.
    pub(crate) fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.bump();
        }
    }

    /// Skip spaces only.
    pub(crate) fn skip_spaces_only(&mut self) {
        while self.peek() == Some(' ') {
            self.bump();
        }
    }

    /// Skip a comment up to, but not including, the line break.
    pub(crate) fn skip_comment(&mut self) {
        while let Some(ch) = self.peek() {
            if is_break(ch) {
                break;
            }
            self.bump();
        }
    }

    /// Consume one line break, if present.
    pub(crate) fn consume_break(&mut self) {
        if matches!(self.peek(), Some('\n') | Some('\r')) {
            self.bump();
        }
    }
}

/// Whether a character ends a line.
pub(crate) fn is_break(ch: char) -> bool {
    ch == '\n' || ch == '\r'
}
