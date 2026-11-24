use std::sync::Mutex;

use akku::dotenv::{self, DotEnv, ErrorKind, ParseOptions};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn parse_export_quotes_comments_and_escapes() {
    let input = "\
\u{FEFF}# top comment\r
export EXPORTED=yes\r
PLAIN=hello # inline comment\r
HASH_INSIDE=foo#bar\r
EMPTY_AFTER_EQ= # comment\r
COLOR=#ff0000\r
SINGLE='line1\r
line2 ${NO_EXPAND} # keep'\r
DOUBLE=\"a\\nb\\rc\\td\\\\\\\"\\${ESCAPED} ${EXPANDED}\"\r
";

    let env = dotenv::parse_str(input).unwrap();
    assert_eq!(env.get("EXPORTED"), Some("yes"));
    assert_eq!(env.get("PLAIN"), Some("hello"));
    assert_eq!(env.get("HASH_INSIDE"), Some("foo#bar"));
    assert_eq!(env.get("EMPTY_AFTER_EQ"), Some(""));
    assert_eq!(env.get("COLOR"), Some("#ff0000"));
    assert_eq!(env.get("SINGLE"), Some("line1\nline2 ${NO_EXPAND} # keep"));
    assert_eq!(
        env.get("DOUBLE"),
        Some("a\nb\rc\td\\\"${ESCAPED} ${EXPANDED}")
    );
    assert_eq!(
        env.keys().collect::<Vec<_>>(),
        vec![
            "EXPORTED",
            "PLAIN",
            "HASH_INSIDE",
            "EMPTY_AFTER_EQ",
            "COLOR",
            "SINGLE",
            "DOUBLE",
        ]
    );

    let no_export = dotenv::parse_str_with(
        "export A=1\n",
        ParseOptions {
            allow_export: false,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(no_export.line(), Some(1));
    assert!(matches!(no_export.kind(), ErrorKind::InvalidKey { .. }));
}

#[test]
fn interpolation_defaults_suppression_and_forward_references() {
    let input = "\
GREETING=${WHO:-world}
FORWARD=${LATER}
EMPTY=
FALLBACK=${EMPTY:-default_val}
NESTED_DEF=${MISSING:-${LATER}/sub}
SINGLE='${LATER}'
ESCAPED=\"\\${LATER} and ${LATER}\"
LATER=akku
";

    let env = dotenv::parse_str(input).unwrap();
    // Raw values remain unexpanded
    assert_eq!(env.get("FORWARD"), Some("${LATER}"));
    assert_eq!(env.get("SINGLE"), Some("${LATER}"));

    let expanded = env.expand().unwrap();
    assert_eq!(expanded.get("GREETING").map(String::as_str), Some("world"));
    assert_eq!(expanded.get("FORWARD").map(String::as_str), Some("akku"));
    assert_eq!(
        expanded.get("FALLBACK").map(String::as_str),
        Some("default_val")
    );
    assert_eq!(
        expanded.get("NESTED_DEF").map(String::as_str),
        Some("akku/sub")
    );
    assert_eq!(expanded.get("SINGLE").map(String::as_str), Some("${LATER}"));
    assert_eq!(
        expanded.get("ESCAPED").map(String::as_str),
        Some("${LATER} and akku")
    );

    assert_eq!(env.expand_value("FORWARD").unwrap(), Some("akku".into()));
    assert_eq!(env.expand_value("NO_SUCH_KEY").unwrap(), None);

    let exp_env = env.expanded().unwrap();
    assert_eq!(exp_env.get("FORWARD"), Some("akku"));
    assert_eq!(exp_env.get("SINGLE"), Some("${LATER}"));
}

#[test]
fn parse_str_never_reads_std_env() {
    // PATH or HOME exist in virtually all environments; parse_str + expand must not read std::env.
    let env = dotenv::parse_str("A=${HOME}\nB=${PATH}\n").unwrap();
    assert_eq!(env.get("A"), Some("${HOME}"));
    let expanded = env.expand().unwrap();
    assert_eq!(expanded.get("A").map(String::as_str), Some(""));
    assert_eq!(expanded.get("B").map(String::as_str), Some(""));

    let strict_err = env
        .expand_with(&ParseOptions {
            strict_interpolation: true,
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(
        strict_err.kind(),
        ErrorKind::UnresolvedVariable { name } if name == "HOME"
    ));
}

#[test]
fn cycle_detection_and_max_depth() {
    let cyclic = dotenv::parse_str("A=${B}\nB=${C}\nC=${A}\n").unwrap();
    let err = cyclic.expand().unwrap_err();
    assert!(matches!(
        err.kind(),
        ErrorKind::CyclicExpansion { chain } if chain == "A -> B -> C -> A"
    ));

    let self_cycle = dotenv::parse_str("X=${X}\n").unwrap();
    let err = self_cycle.expand().unwrap_err();
    assert!(matches!(
        err.kind(),
        ErrorKind::CyclicExpansion { chain } if chain == "X -> X"
    ));

    let deep = dotenv::parse_str("A=${B}\nB=${C}\nC=${D}\nD=leaf\n").unwrap();
    let depth_err = deep
        .expand_with(&ParseOptions {
            max_expansion_depth: 2,
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(
        depth_err.kind(),
        ErrorKind::MaxExpansionDepth { limit: 2 }
    ));

    assert!(deep
        .expand_with(&ParseOptions {
            max_expansion_depth: 3,
            ..Default::default()
        })
        .is_ok());
}

#[test]
fn deeply_nested_defaults_return_max_depth_error() {
    // A bomb like `A=${Z:-${Z:-${Z:-...}}}` with an undefined `Z` previously
    // recursed with a constant depth and overflowed the stack. It must return
    // `MaxExpansionDepth` cleanly once the configured limit is exceeded.
    let mut template = String::from("value");
    for _ in 0..64 {
        template = format!("${{Z:-{template}}}");
    }
    let input = format!("A={template}\n");
    let env = dotenv::parse_str(&input).unwrap();
    let error = env
        .expand_with(&ParseOptions {
            max_expansion_depth: 16,
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(
        error.kind(),
        ErrorKind::MaxExpansionDepth { limit: 16 }
    ));

    // Shallow nesting stays within a larger budget and still expands.
    let shallow = dotenv::parse_str("A=${Z:-${Z:-fine}}\n").unwrap();
    assert_eq!(
        shallow.expand().unwrap().get("A").map(String::as_str),
        Some("fine")
    );
}

#[test]
fn parse_errors_and_line_numbers() {
    let missing_eq = dotenv::parse_str("GOOD=1\nNO_EQUALS\n").unwrap_err();
    assert_eq!(missing_eq.line(), Some(2));
    assert!(matches!(
        missing_eq.kind(),
        ErrorKind::MissingEquals { key } if key == "NO_EQUALS"
    ));

    let bad_key = dotenv::parse_str("123BAD=1\n").unwrap_err();
    assert!(matches!(
        bad_key.kind(),
        ErrorKind::InvalidKey { key } if key == "123BAD"
    ));

    let unterminated = dotenv::parse_str("A='line1\nline2\n").unwrap_err();
    assert_eq!(unterminated.line(), Some(1));
    assert!(matches!(unterminated.kind(), ErrorKind::UnterminatedQuote));

    let bad_esc = dotenv::parse_str("A=\"\\q\"\n").unwrap_err();
    assert!(matches!(
        bad_esc.kind(),
        ErrorKind::InvalidEscape { escape } if escape == "\\q"
    ));
}

#[test]
fn load_into_env_and_overwrite_and_find_and_load() {
    let _guard = ENV_LOCK.lock().unwrap();

    let k_existing = "AKKU_TEST_DOTENV_EXISTING";
    let k_new = "AKKU_TEST_DOTENV_NEW";
    let k_file = "AKKU_TEST_DOTENV_FILE";

    std::env::set_var(k_existing, "original");
    std::env::remove_var(k_new);
    std::env::remove_var(k_file);

    let env = dotenv::parse_str(&format!(
        "{k_existing}=updated\n{k_new}=from_${{{k_existing}}}\n"
    ))
    .unwrap();

    let count = env.load_into_env().unwrap();
    assert_eq!(count, 1);
    assert_eq!(std::env::var(k_existing).unwrap(), "original");
    assert_eq!(std::env::var(k_new).unwrap(), "from_updated");

    let count_overwrite = env.load_into_env_overwrite().unwrap();
    assert_eq!(count_overwrite, 2);
    assert_eq!(std::env::var(k_existing).unwrap(), "updated");

    // Test from_file + find_and_load using temp files
    let dir = std::env::temp_dir();
    let missing_path = dir.join("akku_test_dotenv_does_not_exist_12345.env");
    let present_path = dir.join(format!("akku_test_dotenv_{}.env", std::process::id()));
    std::fs::write(&present_path, format!("{k_file}=loaded_from_file\n")).unwrap();

    let parsed_file = dotenv::from_file(&present_path).unwrap();
    assert_eq!(parsed_file.get(k_file), Some("loaded_from_file"));

    let missing_str = missing_path.to_string_lossy().into_owned();
    let present_str = present_path.to_string_lossy().into_owned();
    let loaded = dotenv::find_and_load(&[&missing_str, &present_str])
        .unwrap()
        .expect("should find second candidate");
    assert_eq!(loaded.get(k_file), Some("loaded_from_file"));
    assert_eq!(std::env::var(k_file).unwrap(), "loaded_from_file");

    assert!(dotenv::find_and_load(&[&missing_str]).unwrap().is_none());

    let _ = std::fs::remove_file(&present_path);
    std::env::remove_var(k_existing);
    std::env::remove_var(k_new);
    std::env::remove_var(k_file);
}

#[test]
fn round_trip_collection_and_merge() {
    let mut env = dotenv::parse_str(
        "A=plain\nB='single ${NO_EXP}'\nC=\"line1\\n\\${ESC} ${A}\"\nD=\"spaced \"\nE=foo#bar\nF=\"foo #bar\"\n",
    )
    .unwrap();

    let serialized = dotenv::write_to_string(&env);
    assert_eq!(serialized, env.to_string());
    let reparsed = dotenv::parse_str(&serialized).unwrap();
    assert_eq!(reparsed, env);
    assert_eq!(reparsed.expand().unwrap(), env.expand().unwrap());

    let other: DotEnv = [
        ("A".to_string(), "overwritten".to_string()),
        ("Z".to_string(), "added".to_string()),
    ]
    .into_iter()
    .collect();
    env.merge(&other);
    assert_eq!(&env["A"], "overwritten");
    assert_eq!(env.get("Z"), Some("added"));
    assert_eq!(env.remove("Z"), Some("added".into()));
    assert!(!env.is_empty());
}
