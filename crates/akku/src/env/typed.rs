//! Typed environment variable getters.
//!
//! Parsing getters never panic: values that do not parse become `None` or
//! the provided default, while [`require`] reports a [`MissingVar`] error.

use std::env;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// ASCII truth list accepted by [`get_bool`]: `1`, `true`, `yes`, `on`.
const TRUTHY: [&str; 4] = ["1", "true", "yes", "on"];
/// ASCII falsy list accepted by [`get_bool`]: `0`, `false`, `no`, `off`.
const FALSY: [&str; 4] = ["0", "false", "no", "off"];

/// The error returned when a required variable is not set.
///
/// # Examples
///
/// ```
/// use akku::env::{MissingVar, require};
///
/// let missing = require("AKKU_DEFINITELY_UNSET").unwrap_err();
/// assert_eq!(missing.key, "AKKU_DEFINITELY_UNSET");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingVar {
    /// The name of the missing variable.
    pub key: String,
}

impl fmt::Display for MissingVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "environment variable is not set: {}", self.key)
    }
}

impl Error for MissingVar {}

/// Fetch one variable as a `String`, or `None` when unset or not UTF-8.
///
/// # Examples
///
/// ```
/// use akku::env::{get_str, set_var};
///
/// set_var("AKKU_TYPED_GREETING", "hello");
/// assert_eq!(get_str("AKKU_TYPED_GREETING").as_deref(), Some("hello"));
/// ```
pub fn get_str(key: &str) -> Option<String> {
    env::var_os(key)?.into_string().ok()
}

/// Fetch one variable as a `String`, falling back to `default`.
///
/// # Examples
///
/// ```
/// use akku::env::{get_str_or, set_var};
///
/// set_var("AKKU_TYPED_LEVEL", "debug");
/// assert_eq!(get_str_or("AKKU_TYPED_LEVEL", "info"), "debug");
/// assert_eq!(get_str_or("AKKU_TYPED_LEVEL_UNSET", "info"), "info");
/// ```
pub fn get_str_or(key: &str, default: &str) -> String {
    get_str(key).unwrap_or_else(|| default.to_owned())
}

/// Parse one variable with [`FromStr`] once it is set.
///
/// Unset variables return `None`; set-but-unparsable values return
/// `Some(Err(...))`.
///
/// # Examples
///
/// ```
/// use akku::env::{get_parsed, set_var};
///
/// set_var("AKKU_TYPED_PORT", "8080");
/// assert_eq!(get_parsed::<u16>("AKKU_TYPED_PORT"), Some(Ok(8080)));
///
/// set_var("AKKU_TYPED_PORT", "oops");
/// assert!(get_parsed::<u16>("AKKU_TYPED_PORT").unwrap().is_err());
/// assert_eq!(get_parsed::<u16>("AKKU_TYPED_PORT_UNSET"), None);
/// ```
pub fn get_parsed<T>(key: &str) -> Option<Result<T, T::Err>>
where
    T: FromStr,
{
    get_str(key).map(|value| T::from_str(&value))
}

/// Parse one variable with [`FromStr`], or use `default` when unset.
///
/// # Errors
///
/// Returns the parse error when a set value does not parse.
pub fn get_parsed_or<T>(key: &str, default: T) -> Result<T, T::Err>
where
    T: FromStr,
{
    match get_str(key) {
        Some(value) => T::from_str(&value),
        None => Ok(default),
    }
}

/// Parse a boolean flag with the conventional truth words.
///
/// `1`, `true`, `yes`, `on` (ASCII case-insensitive, surrounding whitespace
/// ignored) mean `true`; `0`, `false`, `no`, `off` mean `false`; anything
/// else is `None`.
///
/// # Examples
///
/// ```
/// use akku::env::{get_bool, set_var};
///
/// set_var("AKKU_TYPED_FLAG", "On");
/// assert_eq!(get_bool("AKKU_TYPED_FLAG"), Some(true));
/// set_var("AKKU_TYPED_FLAG", "no");
/// assert_eq!(get_bool("AKKU_TYPED_FLAG"), Some(false));
/// set_var("AKKU_TYPED_FLAG", "sometimes");
/// assert_eq!(get_bool("AKKU_TYPED_FLAG"), None);
/// ```
pub fn get_bool(key: &str) -> Option<bool> {
    get_str(key).and_then(|value| parse_bool(&value))
}

/// Parse a boolean flag, falling back to `default` when unset or unparsable.
///
/// # Examples
///
/// ```
/// use akku::env::{get_bool_or, set_var};
///
/// set_var("AKKU_TYPED_FLAG", "yes");
/// assert!(get_bool_or("AKKU_TYPED_FLAG", false));
/// assert!(!get_bool_or("AKKU_TYPED_FLAG_UNSET", false));
/// ```
pub fn get_bool_or(key: &str, default: bool) -> bool {
    get_bool(key).unwrap_or(default)
}

/// Split a variable on the OS path separator, skipping empty entries.
///
/// # Examples
///
/// ```
/// use akku::env::{get_list, set_var};
///
/// // `:` on Unix, `;` on Windows.
/// let sep = if cfg!(windows) { ";" } else { ":" };
/// set_var("AKKU_TYPED_LIST", format!("tools{sep}/usr/bin{sep}"));
/// assert_eq!(get_list("AKKU_TYPED_LIST"), vec!["tools", "/usr/bin"]);
/// assert!(get_list("AKKU_TYPED_LIST_UNSET").is_empty());
/// ```
pub fn get_list(key: &str) -> Vec<String> {
    let Some(value) = env::var_os(key) else {
        return Vec::new();
    };
    env::split_paths(&value)
        .filter(|chunk| !chunk.as_os_str().is_empty())
        .map(|chunk| chunk.to_string_lossy().into_owned())
        .collect()
}

/// Split a comma-separated variable, trimming and skipping empty entries.
///
/// # Examples
///
/// ```
/// use akku::env::{get_csv, set_var};
///
/// set_var("AKKU_TYPED_TAGS", "core, fs ,, process");
/// assert_eq!(get_csv("AKKU_TYPED_TAGS"), vec!["core", "fs", "process"]);
/// ```
pub fn get_csv(key: &str) -> Vec<String> {
    get_str(key)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|chunk| !chunk.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Split a variable on the OS path separator into paths, skipping empties.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use akku::env::{get_paths, set_var};
///
/// let joined = std::env::join_paths(["/bin", "/usr/bin"]).unwrap();
/// set_var("AKKU_TYPED_PATHS", joined);
/// assert_eq!(
///     get_paths("AKKU_TYPED_PATHS"),
///     vec![PathBuf::from("/bin"), PathBuf::from("/usr/bin")]
/// );
/// assert!(get_paths("AKKU_TYPED_PATHS_UNSET").is_empty());
/// ```
pub fn get_paths(key: &str) -> Vec<PathBuf> {
    let Some(value) = env::var_os(key) else {
        return Vec::new();
    };
    env::split_paths(&value)
        .filter(|chunk| !chunk.as_os_str().is_empty())
        .collect()
}

/// Fetch a variable that must be present.
///
/// # Errors
///
/// Returns [`MissingVar`] when the variable is unset or not UTF-8.
pub fn require(key: &str) -> Result<String, MissingVar> {
    get_str(key).ok_or_else(|| MissingVar {
        key: key.to_owned(),
    })
}

fn parse_bool(value: &str) -> Option<bool> {
    let flag = value.trim().to_ascii_lowercase();
    if TRUTHY.contains(&flag.as_str()) {
        Some(true)
    } else if FALSY.contains(&flag.as_str()) {
        Some(false)
    } else {
        None
    }
}
