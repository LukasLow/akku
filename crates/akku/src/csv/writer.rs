//! The CSV writer.

use std::io::Write;

use super::options::{QuoteStyle, Terminator};
use super::{CsvOptions, HeaderRecord, Record, Result};

/// A CSV writer over any [`Write`].
///
/// Fields are quoted only when the configured [`QuoteStyle`] requires it, so
/// the default output is minimal RFC 4180.
///
/// ```
/// use akku::csv::Writer;
///
/// fn main() -> akku::csv::Result<()> {
///     let mut writer = Writer::new(Vec::new());
///     writer.write_record(["x", "y"])?;
///     writer.write_record(["1", "2"])?;
///     assert_eq!(writer.into_string(), "x,y\r\n1,2\r\n");
///     Ok(())
/// }
/// ```
pub struct Writer<W: Write> {
    inner: W,
    options: CsvOptions,
}

impl<W: Write> Writer<W> {
    /// Create a writer with the default RFC 4180 options.
    pub fn new(inner: W) -> Self {
        Self::with_options(inner, CsvOptions::new())
    }

    /// Create a writer with explicit options.
    pub fn with_options(inner: W, options: CsvOptions) -> Self {
        Self { inner, options }
    }

    /// Write one record, appending the configured terminator.
    pub fn write_record<I, T>(&mut self, record: I) -> Result<()>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        self.options.validate()?;
        let mut first = true;
        for field in record {
            if !first {
                self.inner.write_all(&[self.options.delimiter])?;
            }
            first = false;
            self.write_field(field.as_ref())?;
        }
        let terminator: &[u8] = match self.options.terminator {
            Terminator::CrLf => b"\r\n",
            Terminator::Lf => b"\n",
        };
        self.inner.write_all(terminator)?;
        Ok(())
    }

    /// Write a header record.
    pub fn write_headers(&mut self, headers: &HeaderRecord) -> Result<()> {
        self.write_record(headers.iter())
    }

    /// Write every record in `records`.
    pub fn write_all<'a, I>(&mut self, records: I) -> Result<()>
    where
        I: IntoIterator<Item = &'a Record>,
    {
        for record in records {
            self.write_record(record)?;
        }
        Ok(())
    }

    /// Flush the underlying writer.
    pub fn flush(&mut self) -> Result<()> {
        self.inner.flush()?;
        Ok(())
    }

    /// Give the underlying writer back.
    pub fn into_inner(self) -> W {
        self.inner
    }

    fn write_field(&mut self, field: &str) -> Result<()> {
        let quote = self.options.quote;
        let escape = self.options.escape;
        let quoted = match self.options.quote_style {
            QuoteStyle::Always => true,
            QuoteStyle::Never => false,
            QuoteStyle::Necessary => needs_quotes(field, &self.options),
        };

        let mut buffer = Vec::with_capacity(field.len() + 2);
        if quoted {
            buffer.push(quote);
        }
        for &byte in field.as_bytes() {
            // Quote style `Never` writes the field bytes raw: only a quoted
            // field doubles quote characters and escapes.
            if quoted && byte == quote {
                if let Some(escape) = escape.filter(|_| !self.options.double_quote) {
                    buffer.push(escape);
                } else {
                    buffer.push(quote);
                }
                buffer.push(quote);
            } else if quoted && Some(byte) == escape {
                buffer.push(byte);
                buffer.push(byte);
            } else {
                buffer.push(byte);
            }
        }
        if quoted {
            buffer.push(quote);
        }
        self.inner.write_all(&buffer)?;
        Ok(())
    }
}

impl Writer<Vec<u8>> {
    /// Consume the writer and return the written bytes as a UTF-8 string.
    pub fn into_string(self) -> String {
        String::from_utf8_lossy(&self.inner).into_owned()
    }
}

fn needs_quotes(field: &str, options: &CsvOptions) -> bool {
    if field.is_empty() {
        return true;
    }
    if field.starts_with([' ', '\t']) || field.ends_with([' ', '\t']) {
        return true;
    }
    field.bytes().any(|byte| {
        byte == options.delimiter || byte == options.quote || byte == b'\r' || byte == b'\n'
    })
}
