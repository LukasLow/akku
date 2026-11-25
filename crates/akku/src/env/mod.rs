//! Environment access: `std` re-exports, typed getters, and standard
//! application directories.
//!
//! The `env` domain re-exports the `std::env` API and adds typed lookups
//! ([`get_bool`], [`get_parsed`], ...), strict access with [`require`], and
//! conventional per-user directories ([`config_dir`]).
//!
//! # Examples
//!
//! ```
//! use akku::env;
//!
//! std::env::set_var("AKKU_ENV_DOCTEST_PORT", "8080");
//! assert_eq!(env::get_parsed::<u16>("AKKU_ENV_DOCTEST_PORT"), Some(Ok(8080)));
//! assert!(env::require("AKKU_ENV_DOCTEST_PORT").is_ok());
//! ```

pub(crate) mod dirs;
pub(crate) mod typed;

pub use dirs::{cache_base_dir, cache_dir, config_base_dir, config_dir, data_base_dir, data_dir};
pub use typed::{
    get_bool, get_bool_or, get_csv, get_list, get_parsed, get_parsed_or, get_paths, get_str,
    get_str_or, require, MissingVar,
};

// The full stable std::env surface, re-exported explicitly.
// `std::env::home_dir` was un-deprecated in Rust 1.87; allow keeps MSRV 1.85 warning-free.
#[allow(deprecated)]
pub use std::env::home_dir;
pub use std::env::{
    args, args_os, consts, current_dir, current_exe, join_paths, remove_var, set_current_dir,
    set_var, split_paths, temp_dir, var, var_os, vars, vars_os, Args, ArgsOs, JoinPathsError,
    SplitPaths, VarError, Vars, VarsOs,
};
