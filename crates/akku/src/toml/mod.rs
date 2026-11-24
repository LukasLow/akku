//! TOML as its own domain: a TOML v1.0 parser and writer without dependencies.
//!
//! The parser accepts the complete TOML v1.0 syntax and validates the table
//! conflict rules, calendar dates and numeric ranges. The writer produces
//! stable output in insertion order.
//!
//! # Parsing
//!
//! ```
//! use akku::toml::Table;
//!
//! fn main() -> akku::toml::Result<()> {
//!     let table: Table = "[server]\nhost = \"localhost\"\nport = 8080\n".parse()?;
//!     assert_eq!(table.get_str_path("server.host"), Some("localhost"));
//!     assert_eq!(table.get_int_path("server.port"), Some(8080));
//!     Ok(())
//! }
//! ```
//!
//! # Writing
//!
//! ```
//! use akku::toml::{Table, Value};
//!
//! fn main() -> akku::toml::Result<()> {
//!     let mut table = Table::new();
//!     table.insert("name", Value::from("akku"));
//!     table.insert("stable", Value::from(true));
//!
//!     let text = akku::toml::to_string(&table);
//!     assert_eq!(text, "name = \"akku\"\nstable = true\n");
//!     assert_eq!(akku::toml::parse(&text)?, table);
//!     Ok(())
//! }
//! ```

mod datetime;
mod de;
mod error;
mod ser;
mod value;

pub use datetime::{Date, Datetime, Offset, Time};
pub use de::{from_str, parse, parse_with, ParseOptions};
pub use error::{Error, ErrorKind, Position, Result};
pub use ser::{to_string, to_string_with, write_to, SerializeOptions};
pub use value::{Table, Value};
