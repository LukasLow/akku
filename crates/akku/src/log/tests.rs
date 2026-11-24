use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use akku::log::{Field, Format, Level, Logger, Record, Value};
use akku::term::ColorMode;
use akku::time::OffsetDateTime;

#[test]
fn level_ordering_parsing_and_dynamic_filtering() {
    assert!(Level::Trace < Level::Debug);
    assert!(Level::Debug < Level::Info);
    assert!(Level::Info < Level::Warn);
    assert!(Level::Warn < Level::Error);
    assert_eq!("warn".parse::<Level>().unwrap(), Level::Warn);
    assert_eq!("WARNING".parse::<Level>().unwrap(), Level::Warn);
    assert_eq!(Level::Info.to_string(), "INFO");
    assert!("bogus".parse::<Level>().is_err());

    let (logger, buf) = Logger::memory(Level::Warn, Format::Human);
    assert_eq!(logger.min_level(), Level::Warn);
    assert!(!logger.enabled(Level::Info));
    assert!(logger.enabled(Level::Warn));

    akku::log::info!(logger, "ignored");
    akku::log::warn!(logger, "visible");
    assert!(!buf.to_string_lossy().contains("ignored"));
    assert!(buf.to_string_lossy().contains("visible"));

    // Dynamic level change.
    logger.set_level(Level::Debug);
    akku::log::debug!(logger, "now enabled");
    assert!(buf.to_string_lossy().contains("now enabled"));
}

#[test]
fn macros_evaluate_arguments_and_fields_lazily_when_disabled() {
    let (logger, buf) = Logger::memory(Level::Error, Format::Human);
    let side_effects = AtomicUsize::new(0);
    let bump = || side_effects.fetch_add(1, Ordering::SeqCst);

    akku::log::trace!(logger, "t {}", bump());
    akku::log::debug!(logger, "d"; count = bump());
    akku::log::info!(logger, "i {}", bump(); k = bump());
    akku::log::warn!(logger, "w"; k = bump());
    assert_eq!(side_effects.load(Ordering::SeqCst), 0);
    assert!(buf.bytes().is_empty());

    akku::log::error!(logger, "e {}", bump(); k = bump());
    assert_eq!(side_effects.load(Ordering::SeqCst), 2);
    assert!(buf.to_string_lossy().contains("e 0"));
}

#[test]
fn human_and_json_structured_formatting_and_escaping() {
    let ts: OffsetDateTime = "2024-06-01T12:00:00Z".parse().unwrap();

    // JSON format with special characters & all value types.
    let (json_logger, json_buf) = Logger::memory(Level::Trace, Format::Json);
    let rec = Record::new(Level::Info, "app::http", "quoted \"msg\"\nline2")
        .timestamp(ts)
        .field("user", "alice\\bob")
        .field("status", 200u16)
        .field("delta", -5i64)
        .field("ratio", 1.5f64)
        .field("nan_val", f64::NAN)
        .field("ok", true)
        .field("missing", Option::<&str>::None);
    json_logger.log(&rec);

    let line = json_buf.to_string_lossy();
    assert_eq!(line.lines().count(), 1);
    assert!(line.contains("\"timestamp\":\"2024-06-01T12:00:00Z\""));
    assert!(line.contains("\"level\":\"INFO\""));
    assert!(line.contains("\"target\":\"app::http\""));
    assert!(line.contains("\"message\":\"quoted \\\"msg\\\"\\nline2\""));
    assert!(line.contains("\"user\":\"alice\\\\bob\""));
    assert!(line.contains("\"status\":200"));
    assert!(line.contains("\"delta\":-5"));
    assert!(line.contains("\"ratio\":1.5"));
    assert!(line.contains("\"nan_val\":null"));
    assert!(line.contains("\"ok\":true"));
    assert!(line.contains("\"missing\":null"));

    // Human format with color forced Always.
    let color_buf = akku::log::Buffer::new();
    let human_logger = Logger::builder()
        .level(Level::Info)
        .format(Format::Human)
        .color(ColorMode::Always)
        .writer(color_buf.clone())
        .build();
    let fields = [
        Field::new("req_id", Value::from("abc")),
        Field::new("ms", 12u64),
    ];
    let rec2 = Record::new(Level::Warn, "app", "slow query")
        .timestamp(ts)
        .fields(&fields);
    human_logger.log(&rec2);
    let human_out = color_buf.to_string_lossy();
    assert!(human_out.contains("\x1b["));
    assert!(human_out.contains("WARN"));
    assert!(human_out.contains("slow query req_id=abc ms=12"));
}

#[test]
fn concurrent_logging_writes_atomic_uninterleaved_lines() {
    let (logger, buf) = Logger::memory(Level::Info, Format::Json);
    let logger = Arc::new(logger);

    let handles: Vec<_> = (0..8)
        .map(|tid| {
            let l = Arc::clone(&logger);
            thread::spawn(move || {
                for i in 0..50 {
                    akku::log::info!(l, "worker event"; tid = tid, seq = i);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    logger.flush();

    let out = buf.to_string_lossy();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 400);
    for line in lines {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "corrupted line: {line:?}"
        );
    }
}

#[test]
fn global_logger_init_and_macros() {
    let (logger, buf) = Logger::memory(Level::Info, Format::Human);
    akku::log::init_global(logger).unwrap();
    // Second init must fail with InitError.
    let (logger2, _) = Logger::memory(Level::Info, Format::Human);
    assert!(akku::log::init_global(logger2).is_err());

    akku::log::info!("global hello {}", 42; source = "test");
    let out = buf.to_string_lossy();
    assert!(out.contains("global hello 42"));
    assert!(out.contains("source=test"));
}
