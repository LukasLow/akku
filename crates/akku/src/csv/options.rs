//! Reader and writer configuration.

use super::{Error, ErrorKind, Result};

/// How surrounding whitespace is trimmed from fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Trim {
    /// Never trim.
    #[default]
    None,
    /// Trim data records, leave the header record untouched.
    Fields,
    /// Trim the header record, leave data records untouched.
    Headers,
    /// Trim everything.
    All,
}

/// The line terminator used by the writer.
///
/// The reader always accepts both `\r\n` and `\n` regardless of this setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Terminator {
    /// `\r\n`, as required by RFC 4180.
    #[default]
    CrLf,
    /// `\n`.
    Lf,
}

/// When the writer wraps a field in quotes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum QuoteStyle {
    /// Only quote fields that need it.
    #[default]
    Necessary,
    /// Quote every field.
    Always,
    /// Never quote; the caller guarantees the data is safe.
    Never,
}

/// Reader and writer configuration.
///
/// All values have sensible RFC 4180 defaults, so `CsvOptions::new()` is the
/// same as [`CsvOptions::default`].
///
/// ```
/// use akku::csv::CsvOptions;
///
/// let options = CsvOptions::new().delimiter(b';').has_headers(false);
/// assert!(!options.has_headers_enabled());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvOptions {
    /// The field delimiter, `,` by default.
    pub(crate) delimiter: u8,
    /// The quote byte, `"` by default.
    pub(crate) quote: u8,
    /// Whether the first record carries column names.
    pub(crate) has_headers: bool,
    /// Which records are trimmed.
    pub(crate) trim: Trim,
    /// Whether records of different lengths are accepted.
    pub(crate) flexible_fields: bool,
    /// Optional comment byte; a record starting with it is skipped.
    pub(crate) comment: Option<u8>,
    /// Optional escape byte used inside quoted fields.
    pub(crate) escape: Option<u8>,
    /// Whether `""` inside a quoted field means one quote.
    pub(crate) double_quote: bool,
    /// The terminator the writer emits.
    pub(crate) terminator: Terminator,
    /// When the writer quotes fields.
    pub(crate) quote_style: QuoteStyle,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: b'"',
            has_headers: true,
            trim: Trim::None,
            flexible_fields: false,
            comment: None,
            escape: None,
            double_quote: true,
            terminator: Terminator::CrLf,
            quote_style: QuoteStyle::Necessary,
        }
    }
}

impl CsvOptions {
    /// Create the default RFC 4180 configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the field delimiter.
    pub fn delimiter(mut self, delimiter: u8) -> Self {
        self.delimiter = delimiter;
        self
    }

    /// Set the quote byte.
    pub fn quote(mut self, quote: u8) -> Self {
        self.quote = quote;
        self
    }

    /// Set whether the first record is treated as a header record.
    pub fn has_headers(mut self, has_headers: bool) -> Self {
        self.has_headers = has_headers;
        self
    }

    /// Set which records are trimmed.
    pub fn trim(mut self, trim: Trim) -> Self {
        self.trim = trim;
        self
    }

    /// Set whether records may have different lengths.
    pub fn flexible_fields(mut self, flexible_fields: bool) -> Self {
        self.flexible_fields = flexible_fields;
        self
    }

    /// Set the comment byte; lines starting with it are skipped.
    pub fn comment(mut self, comment: Option<u8>) -> Self {
        self.comment = comment;
        self
    }

    /// Set the escape byte used inside quoted fields.
    pub fn escape(mut self, escape: Option<u8>) -> Self {
        self.escape = escape;
        self
    }

    /// Set whether `""` inside a quoted field decodes to a single quote.
    pub fn double_quote(mut self, double_quote: bool) -> Self {
        self.double_quote = double_quote;
        self
    }

    /// Set the terminator the writer emits.
    pub fn terminator(mut self, terminator: Terminator) -> Self {
        self.terminator = terminator;
        self
    }

    /// Set when the writer quotes fields.
    pub fn quote_style(mut self, quote_style: QuoteStyle) -> Self {
        self.quote_style = quote_style;
        self
    }

    /// Whether the reader expects a header record.
    pub fn has_headers_enabled(&self) -> bool {
        self.has_headers
    }

    /// Reject configurations that cannot be parsed or written.
    pub(crate) fn validate(&self) -> Result<()> {
        let invalid = |byte: u8| Error::new(ErrorKind::InvalidDelimiter { byte }, 0);
        if is_line_break(self.quote) {
            return Err(invalid(self.quote));
        }
        if is_line_break(self.delimiter) || self.delimiter == self.quote {
            return Err(invalid(self.delimiter));
        }
        if let Some(comment) = self.comment {
            if is_line_break(comment) || comment == self.delimiter || comment == self.quote {
                return Err(invalid(comment));
            }
        }
        if let Some(escape) = self.escape {
            if escape == self.quote {
                return Err(invalid(escape));
            }
        }
        Ok(())
    }
}

fn is_line_break(byte: u8) -> bool {
    byte == b'\r' || byte == b'\n'
}
