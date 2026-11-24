//! Integration tests for [`akku::error`] — chains, kinds, ergonomics and
//! macros.

use akku::error::{bail, ensure, Context, Error, ErrorKind, Result};
use std::io;
use std::num::ParseIntError;

/// Compile-time assertion helper.
fn assert_send_sync<T: Send + Sync + 'static>() {}

#[test]
fn error_is_send_and_sync() {
    assert_send_sync::<Error>();
}

#[test]
fn context_chain_is_ordered_outermost_first() {
    let error = Error::msg("root").context("middle").context("outer");

    let layers: Vec<String> = error.chain().map(|layer| layer.to_string()).collect();
    assert_eq!(layers, ["outer", "middle", "root"]);

    assert_eq!(error.root_cause().to_string(), "root");
}

#[test]
fn display_is_concise_and_alternate_prints_full_chain() {
    let error = Error::msg("root").context("middle").context("outer");

    assert_eq!(error.to_string(), "outer");
    assert_eq!(format!("{:#}", error), "outer: middle: root");
}

#[test]
fn wrapped_source_error_reports_its_message_once() {
    let source = io::Error::new(io::ErrorKind::NotFound, "missing file");
    let error = Error::new(source)
        .context("reading config")
        .context("booting");

    assert_eq!(error.to_string(), "booting");
    assert_eq!(
        format!("{:#}", error),
        "booting: reading config: missing file"
    );

    let layers: Vec<String> = error.chain().map(|layer| layer.to_string()).collect();
    assert_eq!(layers, ["booting", "reading config", "missing file"]);
}

#[test]
fn debug_contains_every_layer() {
    let error = Error::msg("root").context("outer");
    let debug = format!("{:?}", error);

    assert!(debug.contains("outer"), "debug output: {debug}");
    assert!(debug.contains("root"), "debug output: {debug}");
    assert!(debug.contains("Caused by"), "debug output: {debug}");
}

#[test]
fn downcast_recovers_typed_cause_through_context_layers() {
    let source = io::Error::new(io::ErrorKind::NotFound, "no such file");
    let error = Error::new(source)
        .context("reading config")
        .context("starting server");

    assert!(error.is::<io::Error>());

    let recovered = error
        .downcast_ref::<io::Error>()
        .expect("io error must stay reachable");
    assert_eq!(recovered.kind(), io::ErrorKind::NotFound);
    assert_eq!(error.root_cause().to_string(), "no such file");
}

#[test]
fn error_kind_is_set_and_travels_through_context() {
    assert_eq!(Error::msg("x").kind(), ErrorKind::Other);
    assert_eq!(
        Error::new(io::Error::new(io::ErrorKind::BrokenPipe, "x")).kind(),
        ErrorKind::Other
    );

    let typed = Error::with_kind(ErrorKind::InvalidInput, "bad number");
    let wrapped = typed
        .context("parsing count")
        .with_context(|| format!("while loading {}", "db"));
    assert_eq!(wrapped.kind(), ErrorKind::InvalidInput);
    assert_eq!(wrapped.to_string(), "while loading db");

    assert_eq!(ErrorKind::NotFound.to_string(), "not found");
    assert_eq!(ErrorKind::PermissionDenied.to_string(), "permission denied");
    assert_eq!(ErrorKind::InvalidInput.to_string(), "invalid input");
    assert_eq!(ErrorKind::AlreadyExists.to_string(), "already exists");
    assert_eq!(ErrorKind::TimedOut.to_string(), "timed out");
    assert_eq!(ErrorKind::Unavailable.to_string(), "unavailable");
    assert_eq!(ErrorKind::Unauthenticated.to_string(), "unauthenticated");
    assert_eq!(ErrorKind::Conflict.to_string(), "conflict");
    assert_eq!(ErrorKind::Internal.to_string(), "internal");
    assert_eq!(ErrorKind::Other.to_string(), "other");
}

#[test]
fn context_trait_wraps_result_errors() {
    fn parse_count(input: &str) -> Result<usize> {
        input.parse::<usize>().context("invalid count")
    }

    let error = parse_count("abc").unwrap_err();
    assert_eq!(error.to_string(), "invalid count");
    assert!(error.is::<ParseIntError>());

    let source = io::Error::other("boom");
    let io_result: std::result::Result<u8, io::Error> = Err(source);
    let error = io_result
        .with_context(|| "while writing".to_string())
        .unwrap_err();
    assert_eq!(error.to_string(), "while writing");
    assert!(error.is::<io::Error>());

    let ok: std::result::Result<u8, io::Error> = Ok(7);
    assert_eq!(ok.context("never used").unwrap(), 7);
}

#[test]
fn context_trait_completes_options() {
    let missing: Option<u8> = None;
    let error = missing.context("missing byte").unwrap_err();
    assert_eq!(error.to_string(), "missing byte");
    assert_eq!(error.kind(), ErrorKind::Other);

    let present: Option<u8> = Some(9);
    assert_eq!(
        present.with_context(|| "missing byte".to_string()).unwrap(),
        9
    );

    let none: Option<&str> = None;
    let error = none.with_context(|| format!("key {}", "user")).unwrap_err();
    assert_eq!(error.to_string(), "key user");
}

#[test]
fn bail_macro_returns_formatted_error() {
    fn classify(count: i64) -> Result<&'static str> {
        if count == 0 {
            bail!("count must not be zero");
        }
        if count > 100 {
            bail!("count {count} exceeds the limit of 100");
        }
        Ok("ok")
    }

    assert_eq!(classify(1).unwrap(), "ok");
    assert_eq!(
        classify(0).unwrap_err().to_string(),
        "count must not be zero"
    );
    assert_eq!(
        classify(101).unwrap_err().to_string(),
        "count 101 exceeds the limit of 100",
    );
}

#[test]
fn ensure_macro_checks_conditions() {
    fn divide(dividend: f64, divisor: f64) -> Result<f64> {
        ensure!(divisor != 0.0, "cannot divide {dividend} by zero");
        Ok(dividend / divisor)
    }

    fn positive(value: i64) -> Result<i64> {
        ensure!(value > 0);
        Ok(value)
    }

    assert_eq!(divide(8.0, 2.0).unwrap(), 4.0);
    assert_eq!(
        divide(8.0, 0.0).unwrap_err().to_string(),
        "cannot divide 8 by zero"
    );
    assert_eq!(
        positive(-5).unwrap_err().to_string(),
        "condition failed: value > 0"
    );
    assert_eq!(positive(5).unwrap(), 5);
}

#[test]
fn standard_errors_convert_with_from_impls() {
    let io_error: Error = io::Error::other("io boom").into();
    assert_eq!(io_error.to_string(), "io boom");
    assert!(io_error.is::<io::Error>());

    let fmt_error: Error = std::fmt::Error.into();
    assert!(fmt_error.is::<std::fmt::Error>());

    let invalid_bytes = vec![0xFFu8, 0xFE];
    let utf8_error: Error = std::str::from_utf8(&invalid_bytes).unwrap_err().into();
    assert!(utf8_error.is::<std::str::Utf8Error>());

    let from_utf8_error: Error = String::from_utf8(vec![0xFF]).unwrap_err().into();
    assert!(from_utf8_error.is::<std::string::FromUtf8Error>());

    let parse_int_error: Error = "x".parse::<usize>().unwrap_err().into();
    assert!(parse_int_error.is::<ParseIntError>());

    let parse_float_error: Error = "x".parse::<f64>().unwrap_err().into();
    assert!(parse_float_error.is::<std::num::ParseFloatError>());
}

#[test]
fn question_mark_converts_through_targeted_from_impls() {
    fn parse(input: &str) -> Result<usize> {
        let count: usize = input.parse()?;
        Ok(count + 1)
    }

    assert_eq!(parse("2").unwrap(), 3);
    assert_eq!(
        parse("nope").unwrap_err().to_string(),
        "invalid digit found in string"
    );
}

#[test]
fn context_trait_preserves_the_kind_of_akku_errors() {
    fn load() -> Result<()> {
        Err(Error::with_kind(ErrorKind::NotFound, "user 7"))
    }

    let error = load().context("loading the profile").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.to_string(), "loading the profile");
    assert_eq!(format!("{:#}", error), "loading the profile: user 7");

    let error = load().with_context(|| "described lazily").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(format!("{:#}", error), "described lazily: user 7");
}

#[test]
fn wrapping_an_akku_error_returns_it_unchanged() {
    let inner = Error::new(io::Error::other("boom"));
    let wrapped = Error::new(inner);

    assert_eq!(wrapped.to_string(), "boom");
    assert_eq!(format!("{:#}", wrapped), "boom");
    assert!(wrapped.is::<io::Error>());

    let typed = Error::with_kind(ErrorKind::TimedOut, "slow");
    let wrapped = Error::new(typed);
    assert_eq!(wrapped.kind(), ErrorKind::TimedOut);
    let layers: Vec<String> = wrapped.chain().map(|layer| layer.to_string()).collect();
    assert_eq!(layers, ["slow"], "no duplicate layer is introduced");
}

#[test]
fn source_never_repeats_the_transparent_wrapper() {
    #[derive(Debug)]
    struct Inner;

    impl std::fmt::Display for Inner {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("inner")
        }
    }

    impl std::error::Error for Inner {}

    #[derive(Debug)]
    struct Outer(Inner);

    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("outer")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    let error = Error::new(Outer(Inner));
    assert_eq!(format!("{:#}", error), "outer: inner");

    let next = std::error::Error::source(&error).expect("the wrapper yields the next layer");
    assert_eq!(
        next.to_string(),
        "inner",
        "the wrapper is skipped, not repeated"
    );
    assert!(next.source().is_none());

    let leaf = Error::new(io::Error::other("boom"));
    assert!(
        std::error::Error::source(&leaf).is_none(),
        "a sourceless foreign error has no next layer"
    );

    let layered = Error::msg("root").context("outer");
    let next = std::error::Error::source(&layered).expect("context layers have a source");
    assert_eq!(next.to_string(), "root");
}

#[test]
fn bail_converts_into_custom_error_types() {
    #[derive(Debug)]
    struct CustomErr(Error);

    impl From<Error> for CustomErr {
        fn from(error: Error) -> Self {
            CustomErr(error)
        }
    }

    impl std::fmt::Display for CustomErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Display::fmt(&self.0, f)
        }
    }

    impl std::error::Error for CustomErr {}

    fn check(count: i64) -> std::result::Result<(), CustomErr> {
        if count < 0 {
            bail!("count {count} must not be negative");
        }
        Ok(())
    }

    assert!(check(1).is_ok());
    assert_eq!(
        check(-1).unwrap_err().to_string(),
        "count -1 must not be negative"
    );
}
