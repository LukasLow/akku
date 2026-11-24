//! Standard per-user application directories, as pure path computations.
//!
//! Never creates anything: these helpers only *name* directories. On macOS
//! the `~/Library` layout is used; elsewhere the XDG base directory spec is
//! honored. All overrides are ignored unless they are absolute paths.
//!
//! # Examples
//!
//! ```
//! use akku::env;
//!
//! let base = env::config_dir("myapp");
//! assert!(base.to_string_lossy().contains("myapp"));
//! ```

use std::path::PathBuf;

/// The user's home directory as configured in the environment.
///
/// Returns `$HOME` when it is set and non-empty (`USERPROFILE` on Windows);
/// there is no further fallback, notably no `getpwuid` lookup and no call to
/// the deprecated [`std::env::home_dir`].
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
///
/// let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
/// assert_eq!(
///     akku::env::home_dir(),
///     std::env::var_os(var).map(PathBuf::from)
/// );
/// assert!(akku::env::home_dir().is_some());
/// ```
pub fn home_dir() -> Option<PathBuf> {
    let value = home_var()?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

#[cfg(not(windows))]
fn home_var() -> Option<std::ffi::OsString> {
    std::env::var_os("HOME")
}

#[cfg(windows)]
fn home_var() -> Option<std::ffi::OsString> {
    std::env::var_os("USERPROFILE")
}

/// Base directory for user-specific configuration files.
///
/// On macOS: `~/Library/Application Support`. Elsewhere:
/// `$XDG_CONFIG_HOME` (when absolute), else `~/.config`.
pub fn config_base_dir() -> PathBuf {
    base_dir("Library/Application Support", "XDG_CONFIG_HOME", ".config")
}

/// Base directory for user-specific data files.
///
/// On macOS: `~/Library/Application Support`. Elsewhere:
/// `$XDG_DATA_HOME` (when absolute), else `~/.local/share`.
pub fn data_base_dir() -> PathBuf {
    base_dir(
        "Library/Application Support",
        "XDG_DATA_HOME",
        ".local/share",
    )
}

/// Base directory for disposable cache files.
///
/// On macOS: `~/Library/Caches`. Elsewhere: `$XDG_CACHE_HOME` (when
/// absolute), else `~/.cache`.
pub fn cache_base_dir() -> PathBuf {
    base_dir("Library/Caches", "XDG_CACHE_HOME", ".cache")
}

/// Directory for the configuration files of `app`.
///
/// This is [`config_base_dir`] joined with `app`; the directory is *not*
/// created.
pub fn config_dir(app: &str) -> PathBuf {
    config_base_dir().join(app)
}

/// Directory for the data files of `app`.
///
/// This is [`data_base_dir`] joined with `app`; the directory is *not*
/// created.
pub fn data_dir(app: &str) -> PathBuf {
    data_base_dir().join(app)
}

/// Directory for the cache files of `app`.
///
/// This is [`cache_base_dir`] joined with `app`; the directory is *not*
/// created.
pub fn cache_dir(app: &str) -> PathBuf {
    cache_base_dir().join(app)
}

#[cfg(target_os = "macos")]
fn base_dir(mac_relative: &str, _override_var: &str, _fallback: &str) -> PathBuf {
    join_home(mac_relative)
}

#[cfg(not(target_os = "macos"))]
fn base_dir(_mac_relative: &str, override_var: &str, fallback: &str) -> PathBuf {
    xdg_dir(override_var, fallback)
}

#[cfg(target_os = "macos")]
fn join_home(relative: &str) -> PathBuf {
    match home_dir() {
        Some(home) => home.join(relative),
        None => PathBuf::from(relative),
    }
}

/// XDG override directory when absolute, else `home/<fallback>`.
#[cfg(not(target_os = "macos"))]
fn xdg_dir(override_var: &str, fallback: &str) -> PathBuf {
    if let Some(value) = std::env::var_os(override_var) {
        let candidate = PathBuf::from(value);
        if candidate.is_absolute() {
            return candidate;
        }
    }
    match home_dir() {
        Some(home) => home.join(fallback),
        None => PathBuf::from(fallback),
    }
}
