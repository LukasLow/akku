//! Integration tests for `akku::env`.

use std::error::Error;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use akku::env::get_str;
use akku::env::{
    cache_base_dir, cache_dir, config_base_dir, config_dir, data_base_dir, data_dir, get_bool,
    get_bool_or, get_csv, get_list, get_parsed, get_parsed_or, get_paths, get_str_or, remove_var,
    require, set_var, MissingVar,
};

/// The separator `std::env::split_paths` splits on.
const SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// `akku::env::home_dir` is `std::env::home_dir`, deprecated until Rust 1.87
/// (MSRV is 1.85), so the allow keeps MSRV builds warning-free.
#[allow(deprecated)]
fn test_home() -> PathBuf {
    akku::env::home_dir().expect("the test environment has a home")
}

/// Serialize all environment mutations across the test binaries' threads.
fn lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Set `key` for the duration of `run`, then remove it again.
fn with_env(key: &str, value: &str, run: impl FnOnce()) {
    let _guard = lock();
    remove_var(key);
    set_var(key, value);
    run();
    remove_var(key);
}

#[test]
fn get_bool_accepts_conventional_truth_words() {
    for value in ["1", "true", "True", "YES", "yes", "On", "on"] {
        with_env("AKKU_TEST_BOOL", value, || {
            assert_eq!(get_bool("AKKU_TEST_BOOL"), Some(true), "value {value:?}");
        });
    }
    for value in ["0", "false", "False", "NO", "no", "Off", "off"] {
        with_env("AKKU_TEST_BOOL", value, || {
            assert_eq!(get_bool("AKKU_TEST_BOOL"), Some(false), "value {value:?}");
        });
    }
    for value in ["", "  ", "maybe", "2", "y", "sure"] {
        with_env("AKKU_TEST_BOOL", value, || {
            assert_eq!(get_bool("AKKU_TEST_BOOL"), None, "value {value:?}");
        });
    }
    with_env("AKKU_TEST_BOOL", "", || {
        assert!(get_bool_or("AKKU_TEST_BOOL", true));
        assert!(!get_bool_or("AKKU_TEST_BOOL_UNSET", false));
    });
}

#[test]
fn get_bool_trims_surrounding_whitespace() {
    with_env("AKKU_TEST_BOOL", " yes ", || {
        assert_eq!(get_bool("AKKU_TEST_BOOL"), Some(true));
    });
    with_env("AKKU_TEST_BOOL", "\toff\t", || {
        assert_eq!(get_bool("AKKU_TEST_BOOL"), Some(false));
    });
}

#[test]
fn get_str_and_require_cover_the_basics() {
    with_env("AKKU_TEST_STR", "value", || {
        assert_eq!(get_str("AKKU_TEST_STR").as_deref(), Some("value"));
        assert_eq!(get_str_or("AKKU_TEST_STR", "other"), "value");
        assert_eq!(require("AKKU_TEST_STR").unwrap(), "value");
    });
    with_env("AKKU_TEST_STR", "other", || {
        assert_eq!(get_str_or("AKKU_TEST_MISSING", "other"), "other");
    });
}

#[test]
fn get_parsed_reports_set_unset_and_broken() {
    with_env("AKKU_TEST_PORT", "8080", || {
        assert_eq!(get_parsed::<u16>("AKKU_TEST_PORT"), Some(Ok(8080)));
        assert_eq!(get_parsed_or::<i32>("AKKU_TEST_PORT", -1), Ok(8080));
    });
    with_env("AKKU_TEST_PORT", "not-a-port", || {
        assert!(get_parsed::<u16>("AKKU_TEST_PORT").unwrap().is_err());
        assert!(get_parsed_or::<u16>("AKKU_TEST_PORT", 80).is_err());
    });
    // Unset: no parse attempt, default wins.
    assert_eq!(get_parsed::<u16>("AKKU_TEST_PORT_MISSING"), None);
    assert_eq!(get_parsed_or::<u16>("AKKU_TEST_PORT_MISSING", 42), Ok(42));
}

#[test]
fn get_list_splits_on_the_path_separator() {
    // `;` on Windows, `:` elsewhere (std's split_paths separator).
    let list = format!("tools{SEP}/usr/bin{SEP}{SEP}/usr/local/bin");
    with_env("AKKU_TEST_LIST", &list, || {
        assert_eq!(
            get_list("AKKU_TEST_LIST"),
            vec!["tools", "/usr/bin", "/usr/local/bin"]
        );
        assert_eq!(
            get_paths("AKKU_TEST_LIST"),
            vec![
                PathBuf::from("tools"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    });
    with_env("AKKU_TEST_LIST", SEP, || {
        assert!(get_list("AKKU_TEST_LIST").is_empty());
    });
    // Unset variables yield empty vectors.
    assert!(get_list("AKKU_TEST_LIST_MISSING").is_empty());
    assert!(get_paths("AKKU_TEST_LIST_MISSING").is_empty());
}

#[test]
fn get_csv_trims_and_skips_empties() {
    with_env("AKKU_TEST_CSV", " core , fs ,,process, ", || {
        assert_eq!(get_csv("AKKU_TEST_CSV"), vec!["core", "fs", "process"]);
    });
    with_env("AKKU_TEST_CSV", ",", || {
        assert!(get_csv("AKKU_TEST_CSV").is_empty());
    });
    assert!(get_csv("AKKU_TEST_CSV_MISSING").is_empty());
}

#[test]
fn require_reports_missing_variables() {
    remove_var("AKKU_TEST_REQUIRE_MISSING");
    let missing = require("AKKU_TEST_REQUIRE_MISSING").unwrap_err();
    assert_eq!(
        missing,
        MissingVar {
            key: "AKKU_TEST_REQUIRE_MISSING".to_owned()
        }
    );
    assert_eq!(missing.clone().key, "AKKU_TEST_REQUIRE_MISSING");
    assert_eq!(
        missing.to_string(),
        "environment variable is not set: AKKU_TEST_REQUIRE_MISSING"
    );
    // It behaves as a proper std error.
    let _dyn: &dyn Error = &missing;
    assert!(format!("{missing:?}").contains("MissingVar"));
}

#[cfg(target_os = "macos")]
#[test]
fn mac_uses_the_library_layout() {
    let home = test_home();
    assert_eq!(config_base_dir(), home.join("Library/Application Support"));
    assert_eq!(data_base_dir(), home.join("Library/Application Support"));
    assert_eq!(cache_base_dir(), home.join("Library/Caches"));
    assert_eq!(config_dir("myapp"), config_base_dir().join("myapp"));
    assert_eq!(data_dir("myapp"), data_base_dir().join("myapp"));
    assert_eq!(cache_dir("myapp"), cache_base_dir().join("myapp"));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn unix_honors_absolute_xdg_overrides() {
    use akku::fs::TempDir;

    let dir = TempDir::new("xdg-absolute").unwrap();
    let absolute = dir.as_path().to_path_buf();

    let _guard = lock();
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("XDG_DATA_HOME");
    std::env::remove_var("XDG_CACHE_HOME");

    let home = test_home();
    // No overrides configured: the spec defaults apply.
    assert_eq!(config_base_dir(), home.join(".config"));
    assert_eq!(data_base_dir(), home.join(".local/share"));
    assert_eq!(cache_base_dir(), home.join(".cache"));

    // Relative overrides are ignored.
    set_var("XDG_CONFIG_HOME", "relative/path");
    assert_eq!(config_base_dir(), home.join(".config"));

    // Absolute overrides are honored.
    set_var("XDG_CONFIG_HOME", absolute.join("cfg"));
    set_var("XDG_DATA_HOME", absolute.join("data"));
    set_var("XDG_CACHE_HOME", absolute.join("cache"));
    assert_eq!(config_base_dir(), absolute.join("cfg"));
    assert_eq!(data_base_dir(), absolute.join("data"));
    assert_eq!(cache_base_dir(), absolute.join("cache"));

    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("XDG_DATA_HOME");
    std::env::remove_var("XDG_CACHE_HOME");
}

#[test]
fn app_dirs_are_only_computed_never_created() {
    let _guard = lock();
    let app = "akku-app-dirs-never-create";
    assert!(!akku::fs::try_exists(config_dir(app)).unwrap());
    assert!(!akku::fs::try_exists(data_dir(app)).unwrap());
    assert!(!akku::fs::try_exists(cache_dir(app)).unwrap());
}

#[test]
fn std_env_reexports_are_usable() {
    let _guard = lock();
    set_var("AKKU_TEST_STD_REEXPORT", "v");
    // `var`, `vars`, `vars_os`, `args`, `args_os`, `split_paths`, `join_paths`, `consts`.
    assert_eq!(std::env::var("AKKU_TEST_STD_REEXPORT").unwrap(), "v");
    let vars_iter: akku::env::Vars = akku::env::vars();
    assert!(vars_iter
        .into_iter()
        .any(|(key, _)| key == "AKKU_TEST_STD_REEXPORT"));
    let vars_os_iter: akku::env::VarsOs = akku::env::vars_os();
    assert!(vars_os_iter
        .into_iter()
        .any(|(key, _)| key == "AKKU_TEST_STD_REEXPORT"));

    let _args: akku::env::Args = akku::env::args();
    let _args_os: akku::env::ArgsOs = akku::env::args_os();
    assert!(!akku::env::consts::OS.is_empty());
    assert!(!akku::env::consts::ARCH.is_empty());

    let missing_err: akku::env::VarError =
        akku::env::var("AKKU_TEST_DEFINITELY_MISSING_VAR").unwrap_err();
    assert_eq!(missing_err, std::env::VarError::NotPresent);

    let list = format!("/bin{SEP}/usr/local/bin");
    let split: akku::env::SplitPaths<'_> = akku::env::split_paths(&list);
    let joined = akku::env::join_paths(split).unwrap_or_default();
    assert_eq!(joined, list.as_str());

    // Joining a path that embeds a disallowed character (`:` on Unix, `"` on Windows) triggers JoinPathsError.
    let invalid_entry = if cfg!(windows) {
        r#"bad"entry"#
    } else {
        "bad:entry"
    };
    let join_err: akku::env::JoinPathsError = akku::env::join_paths([invalid_entry]).unwrap_err();
    assert!(!join_err.to_string().is_empty());

    // `akku::env::home_dir` is the exact `std::env::home_dir` function.
    #[allow(deprecated)] // MSRV 1.85 compatibility; un-deprecated in Rust 1.87.
    let std_home = std::env::home_dir();
    #[allow(deprecated)]
    let akku_home = akku::env::home_dir();
    assert_eq!(akku_home, std_home);

    remove_var("AKKU_TEST_STD_REEXPORT");
}
