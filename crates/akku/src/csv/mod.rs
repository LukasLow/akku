//! CSV as its own domain: a streaming RFC 4180 reader and writer.
//!
//! The reader reads records one at a time, reuses the caller's [`Record`]
//! buffers and never allocates a new vector per record. The writer works on
//! any [`std::io::Write`]. Neither needs a dependency.
//!
//! # Reading
//!
//! ```
//! use akku::csv::{Reader, Record};
//!
//! fn main() -> akku::csv::Result<()> {
//!     let data = "name,age\nalice,42\nbob,7\n";
//!     let mut reader = Reader::new(data.as_bytes());
//!     let headers = reader.headers()?.clone();
//!
//!     let mut record = Record::new();
//!     let mut total = 0i64;
//!     while reader.read_record(&mut record)? {
//!         total += record.typed_named::<i64>(&headers, "age")?;
//!     }
//!     assert_eq!(total, 49);
//!     Ok(())
//! }
//! ```
//!
//! # Writing
//!
//! ```
//! use akku::csv::Writer;
//!
//! fn main() -> akku::csv::Result<()> {
//!     let mut writer = Writer::new(Vec::new());
//!     writer.write_record(["a", "b,c"])?;
//!     assert_eq!(writer.into_string(), "a,\"b,c\"\r\n");
//!     Ok(())
//! }
//! ```

mod error;
mod options;
mod reader;
mod writer;

use std::slice;

pub use error::{Error, ErrorKind, Result};
pub use options::{CsvOptions, QuoteStyle, Terminator, Trim};
pub use reader::{Reader, Records};
pub use writer::Writer;

/// A single record: an ordered list of field values.
///
/// Records are reused across reads: [`Reader::read_record`] clears the record
/// and refills it without allocating new field buffers.
///
/// ```
/// use akku::csv::Record;
///
/// let mut record = Record::new();
/// record.push("a");
/// record.push("b");
/// assert_eq!(record.len(), 2);
/// assert_eq!(record.get(1), Some("b"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct Record {
    fields: Vec<String>,
    len: usize,
}

impl Record {
    /// Create an empty record.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of fields in this record.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether this record has no fields.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The field at `index`, if present.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.fields
            .get(index)
            .filter(|_| index < self.len)
            .map(|field| field.as_str())
    }

    /// The field at `index` for mutation, if present.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut String> {
        if index < self.len {
            self.fields.get_mut(index)
        } else {
            None
        }
    }

    /// Iterate over the fields as string slices.
    pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        self.fields[..self.len].iter().map(String::as_str)
    }

    /// All fields as a slice.
    pub fn as_slice(&self) -> &[String] {
        &self.fields[..self.len]
    }

    /// Append a field.
    pub fn push(&mut self, field: impl Into<String>) {
        if self.len < self.fields.len() {
            let slot = &mut self.fields[self.len];
            slot.clear();
            slot.push_str(&field.into());
        } else {
            self.fields.push(field.into());
        }
        self.len += 1;
    }

    /// Drop all fields, keeping the allocated buffers.
    pub fn clear(&mut self) {
        for field in &mut self.fields {
            field.clear();
        }
        self.len = 0;
    }

    /// Parse the field at `index` into `T`.
    pub fn typed<T: FromField>(&self, index: usize) -> Result<T> {
        match self.get(index) {
            Some(field) => T::from_field(field),
            None => Err(missing_field(index)),
        }
    }

    /// The value of the column `name`, looked up through `headers`.
    pub fn get_named<'a>(&'a self, headers: &HeaderRecord, name: &str) -> Result<&'a str> {
        let index = match headers.index(name) {
            Some(index) => index,
            None => {
                return Err(Error::new(
                    ErrorKind::MissingColumn {
                        name: name.to_string(),
                    },
                    0,
                ))
            }
        };
        self.get(index).ok_or_else(|| missing_field(index))
    }

    /// Parse the column `name` into `T`, looked up through `headers`.
    pub fn typed_named<T: FromField>(&self, headers: &HeaderRecord, name: &str) -> Result<T> {
        T::from_field(self.get_named(headers, name)?)
    }

    /// Reserve a field slot for the reader and return it.
    pub(crate) fn slot(&mut self, index: usize) -> &mut String {
        while self.fields.len() <= index {
            self.fields.push(String::new());
        }
        &mut self.fields[index]
    }

    /// Mark the first `count` fields as the content of this record.
    pub(crate) fn commit(&mut self, count: usize) {
        debug_assert!(count <= self.fields.len());
        self.len = count.min(self.fields.len());
    }
}

fn missing_field(index: usize) -> Error {
    Error::new(ErrorKind::MissingField { index }, 0)
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for Record {}

impl<'a> IntoIterator for &'a Record {
    type Item = &'a str;
    type IntoIter = std::iter::Map<slice::Iter<'a, String>, fn(&'a String) -> &'a str>;

    fn into_iter(self) -> Self::IntoIter {
        self.fields[..self.len].iter().map(String::as_str)
    }
}

/// The column names of a CSV input.
///
/// ```
/// use akku::csv::HeaderRecord;
///
/// let headers = HeaderRecord::new(["name", "age"]);
/// assert_eq!(headers.index("age"), Some(1));
/// assert_eq!(headers.index("city"), None);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderRecord {
    names: Vec<String>,
}

impl HeaderRecord {
    /// Create a header record from column names.
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Turn a data record into a header record.
    pub fn from_record(record: Record) -> Self {
        Self {
            names: record.as_slice().to_vec(),
        }
    }

    /// The column names.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The number of columns.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether there are no columns.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The index of the first column named `name`.
    pub fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|column| column == name)
    }

    /// Iterate over the column names.
    pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        self.names.iter().map(String::as_str)
    }
}

/// The current position of a [`Reader`].
///
/// Both counters are zero based for records and one based for lines, so a
/// freshly created reader reports line `1` and record `0`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Position {
    /// The one-based line the reader is currently on.
    pub line: u64,
    /// The number of records read so far.
    pub record: u64,
}

/// Convert a field into a Rust value.
///
/// Implemented for [`String`], the integer types, [`f32`], [`f64`], [`bool`]
/// and [`Option<T>`]. Surrounding whitespace is ignored, and an empty field is
/// `None` for optional types.
///
/// ```
/// use akku::csv::FromField;
///
/// assert_eq!(i64::from_field(" 42 ").unwrap(), 42);
/// assert!(bool::from_field("YES").unwrap());
/// assert_eq!(Option::<i32>::from_field("").unwrap(), None);
/// ```
pub trait FromField: Sized {
    /// The human readable name of the target type, used in error messages.
    fn expected() -> &'static str {
        "field"
    }

    /// Parse `field` into `Self`.
    fn from_field(field: &str) -> Result<Self>;
}

impl FromField for String {
    fn expected() -> &'static str {
        "string"
    }

    fn from_field(field: &str) -> Result<Self> {
        Ok(field.to_string())
    }
}

impl<T: FromField> FromField for Option<T> {
    fn expected() -> &'static str {
        T::expected()
    }

    fn from_field(field: &str) -> Result<Self> {
        if field.trim().is_empty() {
            Ok(None)
        } else {
            T::from_field(field).map(Some)
        }
    }
}

impl FromField for bool {
    fn expected() -> &'static str {
        "boolean"
    }

    fn from_field(field: &str) -> Result<Self> {
        let trimmed = field.trim();
        if trimmed.eq_ignore_ascii_case("true")
            || trimmed.eq_ignore_ascii_case("1")
            || trimmed.eq_ignore_ascii_case("yes")
            || trimmed.eq_ignore_ascii_case("on")
        {
            return Ok(true);
        }
        if trimmed.eq_ignore_ascii_case("false")
            || trimmed.eq_ignore_ascii_case("0")
            || trimmed.eq_ignore_ascii_case("no")
            || trimmed.eq_ignore_ascii_case("off")
        {
            return Ok(false);
        }
        Err(parse_field("boolean", trimmed))
    }
}

fn parse_field(expected: &'static str, field: &str) -> Error {
    Error::new(
        ErrorKind::ParseField {
            expected,
            field: field.to_string(),
        },
        0,
    )
}

macro_rules! impl_integer_field {
    ($($type:ty),* $(,)?) => {
        $(
            impl FromField for $type {
                fn expected() -> &'static str {
                    "integer"
                }

                fn from_field(field: &str) -> Result<Self> {
                    let trimmed = field.trim();
                    trimmed
                        .parse::<$type>()
                        .map_err(|_| parse_field("integer", trimmed))
                }
            }
        )*
    };
}

impl_integer_field!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

macro_rules! impl_float_field {
    ($($type:ty),* $(,)?) => {
        $(
            impl FromField for $type {
                fn expected() -> &'static str {
                    "float"
                }

                fn from_field(field: &str) -> Result<Self> {
                    let trimmed = field.trim();
                    trimmed
                        .parse::<$type>()
                        .map_err(|_| parse_field("float", trimmed))
                }
            }
        )*
    };
}

impl_float_field!(f32, f64);

/// Whether `byte` ends a line.
pub(crate) fn is_line_break(byte: u8) -> bool {
    byte == b'\r' || byte == b'\n'
}
