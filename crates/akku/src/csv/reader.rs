//! The streaming CSV reader.

use std::io::BufRead;

use super::options::Trim;
use super::{is_line_break, CsvOptions, Error, ErrorKind, HeaderRecord, Position, Record, Result};

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// A streaming CSV reader.
///
/// The reader pulls bytes through [`BufRead::fill_buf`], so it works on any
/// buffered source and never loads the whole input. The parser keeps its
/// intermediate state on the reader, which makes it correct across buffer
/// boundaries of any size, including one byte of lookahead.
///
/// ```
/// use akku::csv::Reader;
///
/// fn main() -> akku::csv::Result<()> {
///     let mut reader = Reader::new("a,b\n1,2\n".as_bytes());
///     let records = reader.read_all()?;
///     assert_eq!(records.len(), 1);
///     assert_eq!(records[0].get(1), Some("2"));
///     Ok(())
/// }
/// ```
pub struct Reader<R> {
    inner: R,
    options: CsvOptions,
    headers: HeaderRecord,
    header_record: Record,
    headers_ready: bool,
    bom_checked: bool,
    expected_len: Option<usize>,
    pushback: Vec<u8>,
    utf8: Vec<u8>,
    field_scratch: String,
    line: u64,
    record: u64,
}

impl<R: BufRead> Reader<R> {
    /// Create a reader with the default RFC 4180 options.
    pub fn new(inner: R) -> Self {
        Self::with_options(inner, CsvOptions::new())
    }

    /// Create a reader with explicit options.
    pub fn with_options(inner: R, options: CsvOptions) -> Self {
        Self {
            inner,
            options,
            headers: HeaderRecord::default(),
            header_record: Record::new(),
            headers_ready: false,
            bom_checked: false,
            expected_len: None,
            pushback: Vec::new(),
            utf8: Vec::new(),
            field_scratch: String::new(),
            line: 1,
            record: 0,
        }
    }

    /// The column names.
    ///
    /// The header record is read once, on the first call. If the reader was
    /// configured without headers, the returned record is empty.
    pub fn headers(&mut self) -> Result<&HeaderRecord> {
        self.ensure_headers()?;
        Ok(&self.headers)
    }

    /// Read the next record into `record`.
    ///
    /// Returns `false` at the end of the input. The record is cleared first and
    /// its field buffers are reused, so a loop over `read_record` does not
    /// allocate per record.
    pub fn read_record(&mut self, record: &mut Record) -> Result<bool> {
        self.options.validate()?;
        self.ensure_headers()?;
        if !self.read_raw_record(record, false)? {
            return Ok(false);
        }
        if self.options.flexible_fields {
            return Ok(true);
        }
        let expected = match self.expected_len {
            Some(expected) => expected,
            None => {
                self.expected_len = Some(record.len());
                return Ok(true);
            }
        };
        if record.len() != expected {
            return Err(Error::new(
                ErrorKind::UnequalLengths {
                    expected,
                    got: record.len(),
                },
                self.line,
            ));
        }
        Ok(true)
    }

    /// Read every remaining record into a vector.
    pub fn read_all(&mut self) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        let mut record = Record::new();
        while self.read_record(&mut record)? {
            records.push(std::mem::take(&mut record));
        }
        Ok(records)
    }

    /// Turn this reader into an iterator over records.
    pub fn records(self) -> Records<R> {
        Records {
            reader: self,
            record: Record::new(),
            done: false,
        }
    }

    /// The current position of the reader.
    pub fn position(&self) -> Position {
        Position {
            line: self.line,
            record: self.record,
        }
    }

    /// Give the underlying reader back.
    pub fn into_inner(self) -> R {
        self.inner
    }

    fn ensure_headers(&mut self) -> Result<()> {
        if self.headers_ready {
            return Ok(());
        }
        self.headers_ready = true;
        self.options.validate()?;
        self.check_bom()?;
        if !self.options.has_headers {
            return Ok(());
        }
        let mut record = std::mem::take(&mut self.header_record);
        if self.read_raw_record(&mut record, true)? {
            if !self.options.flexible_fields {
                self.expected_len = Some(record.len());
            }
            self.headers = HeaderRecord::from_record(record);
        }
        Ok(())
    }

    fn check_bom(&mut self) -> Result<()> {
        if self.bom_checked {
            return Ok(());
        }
        self.bom_checked = true;
        let mut start = Vec::with_capacity(BOM.len());
        while start.len() < BOM.len() {
            let byte = {
                let buffer = self.inner.fill_buf()?;
                match buffer.first() {
                    Some(byte) => *byte,
                    None => break,
                }
            };
            self.inner.consume(1);
            start.push(byte);
        }
        if start == BOM {
            return Ok(());
        }
        for byte in start.into_iter().rev() {
            self.pushback.push(byte);
        }
        Ok(())
    }

    fn peek_byte(&mut self) -> Result<Option<u8>> {
        if let Some(byte) = self.pushback.last() {
            return Ok(Some(*byte));
        }
        let byte = {
            let buffer = self.inner.fill_buf()?;
            match buffer.first() {
                Some(byte) => *byte,
                None => return Ok(None),
            }
        };
        self.inner.consume(1);
        self.pushback.push(byte);
        Ok(Some(byte))
    }

    fn next_byte(&mut self) -> Result<Option<u8>> {
        let byte = self.peek_byte()?;
        if let Some(byte) = byte {
            self.pushback.pop();
            if byte == b'\n' {
                self.line += 1;
            }
        }
        Ok(byte)
    }

    fn skip_line(&mut self) -> Result<()> {
        while let Some(byte) = self.next_byte()? {
            if byte == b'\n' {
                break;
            }
        }
        Ok(())
    }

    fn push_byte(&mut self, byte: u8) -> Result<()> {
        if byte < 0x80 {
            self.flush_utf8()?;
            self.field_scratch.push(byte as char);
        } else {
            self.utf8.push(byte);
        }
        Ok(())
    }

    fn flush_utf8(&mut self) -> Result<()> {
        if self.utf8.is_empty() {
            return Ok(());
        }
        let line = self.line;
        let piece = std::str::from_utf8(&self.utf8)
            .map_err(|_| Error::new(ErrorKind::InvalidUtf8, line))?;
        self.field_scratch.push_str(piece);
        self.utf8.clear();
        Ok(())
    }

    fn read_raw_record(&mut self, record: &mut Record, is_header: bool) -> Result<bool> {
        record.clear();
        loop {
            match self.peek_byte()? {
                None => return Ok(false),
                Some(byte) if is_line_break(byte) => {
                    self.next_byte()?;
                }
                Some(byte) if Some(byte) == self.options.comment => {
                    self.skip_line()?;
                }
                Some(_) => break,
            }
        }

        let mut index = 0usize;
        loop {
            self.read_field(record, index, is_header)?;
            index += 1;
            match self.peek_byte()? {
                None => break,
                Some(byte) if byte == self.options.delimiter => {
                    self.next_byte()?;
                }
                Some(byte) if is_line_break(byte) => {
                    self.next_byte()?;
                    if byte == b'\r' && self.peek_byte()? == Some(b'\n') {
                        self.next_byte()?;
                    }
                    break;
                }
                Some(_) => break,
            }
        }
        record.commit(index);
        self.record += 1;
        Ok(true)
    }

    fn read_field(&mut self, record: &mut Record, index: usize, is_header: bool) -> Result<bool> {
        self.field_scratch.clear();
        self.utf8.clear();
        let quote = self.options.quote;
        let quoted = self.peek_byte()? == Some(quote);
        if quoted {
            self.next_byte()?;
            loop {
                let byte = match self.next_byte()? {
                    Some(byte) => byte,
                    None => return Err(Error::new(ErrorKind::UnterminatedQuote, self.line)),
                };
                if byte == quote {
                    if self.options.double_quote && self.peek_byte()? == Some(quote) {
                        self.next_byte()?;
                        self.push_byte(quote)?;
                    } else {
                        break;
                    }
                } else if Some(byte) == self.options.escape {
                    match self.next_byte()? {
                        Some(escaped) => self.push_byte(escaped)?,
                        None => return Err(Error::new(ErrorKind::UnterminatedQuote, self.line)),
                    }
                } else {
                    self.push_byte(byte)?;
                }
            }
            // Horizontal whitespace after the closing quote is allowed, but
            // any other content that is neither the delimiter nor a line
            // break silently started a new record — reject it instead.
            while let Some(byte) = self.peek_byte()? {
                if byte == b' ' || byte == b'\t' {
                    self.next_byte()?;
                } else {
                    break;
                }
            }
            if let Some(byte) = self.peek_byte()? {
                if byte != self.options.delimiter && !is_line_break(byte) {
                    return Err(Error::new(
                        ErrorKind::UnexpectedAfterQuote { byte },
                        self.line,
                    ));
                }
            }
        } else {
            loop {
                match self.peek_byte()? {
                    None => break,
                    Some(byte) if byte == self.options.delimiter || is_line_break(byte) => break,
                    Some(_) => {
                        let byte = self.next_byte()?.unwrap_or_default();
                        self.push_byte(byte)?;
                    }
                }
            }
        }
        self.flush_utf8()?;

        let trim = match self.options.trim {
            Trim::None => false,
            Trim::Fields => !is_header,
            Trim::Headers => is_header,
            Trim::All => true,
        };
        if trim && !quoted {
            let trimmed = self.field_scratch.trim();
            if trimmed.len() != self.field_scratch.len() {
                let value = trimmed.to_string();
                self.field_scratch = value;
            }
        }

        let slot = record.slot(index);
        std::mem::swap(slot, &mut self.field_scratch);
        Ok(quoted)
    }
}

/// An iterator over the records of a [`Reader`].
pub struct Records<R> {
    reader: Reader<R>,
    record: Record,
    done: bool,
}

impl<R: BufRead> Iterator for Records<R> {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.reader.read_record(&mut self.record) {
            Ok(true) => {
                let mut record = Record::new();
                std::mem::swap(&mut record, &mut self.record);
                Some(Ok(record))
            }
            Ok(false) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }
}
