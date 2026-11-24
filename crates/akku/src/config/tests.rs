//! Public-API tests for `akku::config`.
//!
//! These tests exercise the layering contract, the coercion rules and the
//! error surface from the outside, exactly as an application would.

use std::collections::BTreeMap;
use std::path::PathBuf;

use akku::config::{builder, Builder, Config, ConfigValue, ErrorKind, Source};

// ---------------------------------------------------------------- layering --

/// Build the stack `defaults < toml < json < ini < dotenv < env < set`, but stop
/// after `through` layers (`0` means the defaults layer alone).
fn layered(through: usize) -> Config {
    let mut stack = builder().add_defaults().set_default("mode", "defaults");
    if through >= 1 {
        stack = stack.add_toml("mode = \"toml\"\n").unwrap();
    }
    if through >= 2 {
        stack = stack.add_json(r#"{"mode": "json"}"#).unwrap();
    }
    if through >= 3 {
        stack = stack.add_ini("mode = ini\n").unwrap();
    }
    if through >= 4 {
        stack = stack.add_dotenv("mode=dotenv\n").unwrap();
    }
    if through >= 5 {
        stack = stack.add_env_iter("APP", [("APP_MODE", "env")]);
    }
    if through >= 6 {
        stack = stack.set("mode", "explicit");
    }
    stack.build()
}

#[test]
fn each_layer_wins_over_the_layer_below_it() {
    let expected = [
        ("defaults", Source::Defaults),
        ("toml", Source::Toml),
        ("json", Source::Json),
        ("ini", Source::Ini),
        ("dotenv", Source::DotEnv),
        ("env", Source::Env),
        ("explicit", Source::Explicit),
    ];

    for (through, (value, source)) in expected.iter().enumerate() {
        let config = layered(through);
        assert_eq!(
            config.require_str("mode").unwrap(),
            *value,
            "winner at layer {through}"
        );
        assert_eq!(
            config.source_of("mode"),
            Some(source),
            "source at layer {through}"
        );
    }
}

#[test]
fn swapping_the_layer_order_swaps_the_winner() {
    let toml_last = builder()
        .add_json(r#"{"mode": "json"}"#)
        .unwrap()
        .add_toml("mode = \"toml\"\n")
        .unwrap()
        .build();
    assert_eq!(toml_last.require_str("mode").unwrap(), "toml");
    assert_eq!(toml_last.source_of("mode"), Some(&Source::Toml));

    let json_last = builder()
        .add_toml("mode = \"toml\"\n")
        .unwrap()
        .add_json(r#"{"mode": "json"}"#)
        .unwrap()
        .build();
    assert_eq!(json_last.require_str("mode").unwrap(), "json");
    assert_eq!(json_last.source_of("mode"), Some(&Source::Json));
}

#[test]
fn tables_merge_leaf_wise_but_lists_are_replaced() {
    let config = builder()
        .add_toml("[db]\nhost = \"localhost\"\nport = 5432\nports = [1, 2, 3]\n")
        .unwrap()
        .add_env_iter("APP", [("APP_DB__PORT", "6543"), ("APP_DB__PORTS", "9")])
        .build();

    // The sibling key of the overridden one survives.
    assert_eq!(config.require_str("db.host").unwrap(), "localhost");
    assert_eq!(config.source_of("db.host"), Some(&Source::Toml));
    assert_eq!(config.require_int("db.port").unwrap(), 6543);
    assert_eq!(config.source_of("db.port"), Some(&Source::Env));

    // The list is replaced as a whole, not concatenated.
    assert_eq!(config.require_list("db.ports").unwrap(), vec!["9"]);
    assert_eq!(config.source_of("db.ports"), Some(&Source::Env));
    assert_eq!(config.require_table("db").unwrap().len(), 3);

    let replaced_by_list = builder()
        .add_toml("[db]\nports = [1, 2, 3]\n")
        .unwrap()
        .add_json(r#"{"db": {"ports": ["a", "b"]}}"#)
        .unwrap()
        .build();
    assert_eq!(
        replaced_by_list.require_list("db.ports").unwrap(),
        vec!["a", "b"]
    );
}

#[test]
fn a_later_table_replaces_a_scalar_at_the_same_path() {
    let config = builder()
        .add_json(r#"{"db": "not-a-table"}"#)
        .unwrap()
        .add_toml("[db]\nhost = \"localhost\"\n")
        .unwrap()
        .build();
    assert_eq!(config.require_str("db.host").unwrap(), "localhost");
    assert_eq!(config.source_of("db.host"), Some(&Source::Toml));

    let config = builder()
        .add_toml("[db]\nhost = \"localhost\"\n")
        .unwrap()
        .add_json(r#"{"db": "flat"}"#)
        .unwrap()
        .build();
    assert_eq!(config.require_str("db").unwrap(), "flat");
    assert_eq!(config.source_of("db"), Some(&Source::Json));
    assert!(config.get("db.host").is_none());
}

// ------------------------------------------------------------ environment --

#[test]
fn env_prefix_maps_double_underscore_to_a_dot() {
    let config = builder()
        .add_env_iter(
            "APP",
            [
                ("APP_SERVER__PORT", "8080"),
                ("APP_SERVER_PORT", "1"),
                ("APPLE", "nope"),
                ("OTHER_SERVER__PORT", "1111"),
            ],
        )
        .build();

    assert_eq!(config.require_int("server.port").unwrap(), 8080);
    assert_eq!(config.source_of("server.port"), Some(&Source::Env));
    assert_eq!(config.require_int("server_port").unwrap(), 1);
    assert!(config.get("apple").is_none());
    assert!(config.get("le").is_none());
    assert!(config.get("other.server.port").is_none());
}

#[test]
fn a_trailing_underscore_in_the_prefix_makes_no_difference() {
    let plain = builder()
        .add_env_iter("APP", [("APP_SERVER__PORT", "8080")])
        .build();
    let underscored = builder()
        .add_env_iter("APP_", [("APP_SERVER__PORT", "8080")])
        .build();

    assert_eq!(plain.to_map(), underscored.to_map());
    assert_eq!(
        plain.source_of("server.port"),
        underscored.source_of("server.port")
    );
}

#[test]
fn add_env_prefix_reads_a_snapshot_and_ignores_foreign_prefixes() {
    let config = builder()
        .add_env_prefix("AKKU_CONFIG_TEST_PREFIX_THAT_DOES_NOT_EXIST_9E7F")
        .build();
    assert!(config.keys().next().is_none());
}

// --------------------------------------------------------------- coercion --

#[test]
fn getters_coerce_the_usual_scalar_spellings() {
    let config = builder()
        .add_json(
            r#"{
                "port": " 8080 ",
                "version": "4.2",
                "count": 3,
                "ratio": 0.5,
                "enabled": true,
                "flag": "YES",
                "disabled": "0",
                "maybe": "perhaps",
                "name": "akku",
                "tags": ["a", 2, true],
                "csv": " one , two ,, three "
            }"#,
        )
        .unwrap()
        .build();

    assert_eq!(config.get_int("port"), Some(8080));
    assert_eq!(config.get_int("version"), None);
    assert_eq!(config.get_int("enabled"), None);

    assert_eq!(config.get_float("version"), Some(4.2));
    assert_eq!(config.get_float("count"), Some(3.0));
    assert_eq!(config.get_float("ratio"), Some(0.5));
    assert_eq!(config.get_float("name"), None);

    assert_eq!(config.get_bool("enabled"), Some(true));
    assert_eq!(config.get_bool("flag"), Some(true));
    assert_eq!(config.get_bool("disabled"), Some(false));
    assert_eq!(config.get_bool("maybe"), None);
    assert_eq!(config.get_bool("port"), None);

    assert_eq!(config.get_str("name"), Some("akku"));
    assert_eq!(config.get_str("count"), None);

    assert_eq!(
        config.get_list("tags"),
        Some(vec!["a".to_string(), "2".to_string(), "true".to_string()])
    );
    assert_eq!(
        config.get_list("csv"),
        Some(vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string()
        ])
    );
    assert_eq!(config.get_list("name"), Some(vec!["akku".to_string()]));
}

#[test]
fn require_errors_name_the_path_and_the_types() {
    let config = builder()
        .add_json(r#"{"server": {"port": "not-a-number", "host": "localhost"}}"#)
        .unwrap()
        .build();

    let missing = config.require_int("server.timeout").unwrap_err();
    assert!(
        missing.to_string().contains("server.timeout"),
        "message was {missing}"
    );
    match missing.kind() {
        ErrorKind::Missing { path } => assert_eq!(path, "server.timeout"),
        other => panic!("expected Missing, got {other:?}"),
    }

    let wrong = config.require_int("server.port").unwrap_err();
    assert!(
        wrong.to_string().contains("server.port"),
        "message was {wrong}"
    );
    match wrong.kind() {
        ErrorKind::WrongType {
            path,
            expected,
            found,
        } => {
            assert_eq!(path, "server.port");
            assert_eq!(*expected, "int");
            assert_eq!(*found, "string");
        }
        other => panic!("expected WrongType, got {other:?}"),
    }

    assert_eq!(config.require_str("server.host").unwrap(), "localhost");
    assert!(config.require_str("server").is_err());
    assert!(config.require_float("server.host").is_err());
    assert!(config.require_bool("server.host").is_err());
    assert!(config.require_list("server").is_err());
    assert!(config.require_table("server.host").is_err());
    assert!(config.require_table("server").is_ok());
    assert!(config.require_table("absent").is_err());
}

#[test]
fn coercion_works_for_require_getters_too() {
    let config = builder()
        .add_dotenv("PORT=8080\nRATIO=0.25\nDEBUG=on\nTAGS=a,b\n")
        .unwrap()
        .build();

    assert_eq!(config.require_int("PORT").unwrap(), 8080);
    assert_eq!(config.require_float("RATIO").unwrap(), 0.25);
    assert!(config.require_bool("DEBUG").unwrap());
    assert_eq!(
        config.require_list("TAGS").unwrap(),
        vec!["a".to_string(), "b".to_string()]
    );
}

// ------------------------------------------------------------------ files --

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("akku-config-{}-{tag}", std::process::id()))
}

#[test]
fn file_layers_load_and_optional_variants_ignore_missing_files() {
    let json_path = temp_path("settings.json");
    std::fs::write(&json_path, r#"{"json_key": "from-json"}"#).unwrap();
    let toml_path = temp_path("settings.toml");
    std::fs::write(&toml_path, "toml_key = \"from-toml\"\n").unwrap();
    let ini_path = temp_path("settings.ini");
    std::fs::write(&ini_path, "ini_key = from-ini\n").unwrap();
    let dotenv_path = temp_path("settings.env");
    std::fs::write(&dotenv_path, "DOTENV_KEY=from-dotenv\n").unwrap();
    let missing = temp_path("settings-absent.json");

    let config = builder()
        .add_json_file(&json_path)
        .unwrap()
        .add_json_file_optional(&missing)
        .unwrap()
        .add_toml_file(&toml_path)
        .unwrap()
        .add_toml_file_optional(&missing)
        .unwrap()
        .add_ini_file(&ini_path)
        .unwrap()
        .add_dotenv_file(&dotenv_path)
        .unwrap()
        .build();

    assert_eq!(config.require_str("json_key").unwrap(), "from-json");
    assert_eq!(config.require_str("toml_key").unwrap(), "from-toml");
    assert_eq!(config.require_str("ini_key").unwrap(), "from-ini");
    assert_eq!(config.require_str("DOTENV_KEY").unwrap(), "from-dotenv");
    assert_eq!(config.keys().count(), 4);

    let error = builder().add_json_file(&missing).unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Io(_)));
    assert!(std::error::Error::source(&error).is_some());
    assert!(builder().add_toml_file(&missing).is_err());
    assert!(builder().add_ini_file(&missing).is_err());
    assert!(builder().add_dotenv_file(&missing).is_err());

    let _ = std::fs::remove_file(&json_path);
    let _ = std::fs::remove_file(&toml_path);
    let _ = std::fs::remove_file(&ini_path);
    let _ = std::fs::remove_file(&dotenv_path);
}

#[test]
fn already_parsed_values_can_be_added() {
    let json = akku::json::parse(r#"{"from_json": 1}"#).unwrap();
    let toml = akku::toml::parse("[from_toml]\nflag = true\n").unwrap();

    let config = builder()
        .add_json_value(&json)
        .add_toml_table(&toml)
        .build();
    assert_eq!(config.require_int("from_json").unwrap(), 1);
    assert!(config.require_bool("from_toml.flag").unwrap());

    // A non-object JSON root is not a configuration layer.
    let scalar = akku::json::parse("42").unwrap();
    let unchanged = builder().add_json_value(&scalar).build();
    assert!(unchanged.keys().next().is_none());
}

// ---------------------------------------------------------------- formats --

fn format_error(result: akku::config::Result<Builder>) -> (Source, usize, String) {
    match result {
        Err(error) => match error.kind() {
            ErrorKind::Format {
                source,
                line,
                reason,
            } => (
                *source,
                line.expect("format errors keep a line"),
                reason.clone(),
            ),
            other => panic!("expected a format error, got {other:?}"),
        },
        Ok(_) => panic!("expected the layer to fail"),
    }
}

#[test]
fn layer_parsers_keep_their_source_and_line() {
    let (source, line, _) = format_error(builder().add_json("{\n  \"a\": 1,\n}"));
    assert_eq!((source, line), (Source::Json, 3));

    let (source, line, reason) = format_error(builder().add_toml("a = 1\na = 2\n"));
    assert_eq!((source, line), (Source::Toml, 2));
    assert!(reason.contains("duplicate"), "reason was {reason}");

    let (source, line, reason) = format_error(builder().add_ini("a = 1\nbroken line\n"));
    assert_eq!((source, line), (Source::Ini, 2));
    assert!(reason.contains("delimiter"), "reason was {reason}");

    let (source, line, reason) = format_error(builder().add_dotenv("A=1\nBROKEN\n"));
    assert_eq!((source, line), (Source::DotEnv, 2));
    assert!(reason.contains("BROKEN"), "reason was {reason}");

    let error = builder().add_json("{\n  \"a\": 1,\n}").unwrap_err();
    assert!(error.to_string().contains("line 3"), "message was {error}");

    // A non-object root is a model error, not a parser error.
    let error = builder().add_json("[]").unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Parse { .. }));
}

#[test]
fn toml_dates_survive_as_strings() {
    let config = builder()
        .add_toml("started = 1979-05-27T07:32:00Z\n")
        .unwrap()
        .build();
    assert_eq!(
        config.require_str("started").unwrap(),
        "1979-05-27T07:32:00Z"
    );
}

#[test]
fn ini_sections_become_tables() {
    let config = builder()
        .add_ini("root = value\n[server]\nhost = localhost\nport = 8080\n")
        .unwrap()
        .build();
    assert_eq!(config.require_str("root").unwrap(), "value");
    assert_eq!(config.require_str("server.host").unwrap(), "localhost");
    assert_eq!(config.require_int("server.port").unwrap(), 8080);
    assert_eq!(config.source_of("server.port"), Some(&Source::Ini));
}

// ------------------------------------------------------------ config type --

#[test]
fn config_is_clone_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_clone<T: Clone>() {}
    assert_send_sync::<Config>();
    assert_clone::<Config>();
    assert_send_sync::<ConfigValue>();
    assert_clone::<ConfigValue>();

    let config = builder().set("a", 1).build();
    let clone = config.clone();
    assert_eq!(clone.to_map(), config.to_map());
    assert_eq!(clone.require_int("a").unwrap(), 1);
}

#[test]
fn merge_and_with_default_respect_existing_values() {
    let mut low = builder()
        .add_toml("[db]\nhost = \"low\"\nport = 1\n")
        .unwrap()
        .build();
    let high = builder()
        .add_env_iter("APP", [("APP_DB__PORT", "2")])
        .build();
    low.merge(high);

    assert_eq!(low.require_str("db.host").unwrap(), "low");
    assert_eq!(low.source_of("db.host"), Some(&Source::Toml));
    assert_eq!(low.require_int("db.port").unwrap(), 2);
    assert_eq!(low.source_of("db.port"), Some(&Source::Env));

    let config = low
        .with_default("db.host", "shadowed")
        .with_default("db.name", "main");
    assert_eq!(config.require_str("db.host").unwrap(), "low");
    assert_eq!(config.require_str("db.name").unwrap(), "main");
    assert_eq!(config.source_of("db.name"), Some(&Source::Defaults));
}

#[test]
fn keys_and_maps_expose_the_top_level() {
    let config = builder()
        .add_json(r#"{"b": 1, "a": {"x": 2}}"#)
        .unwrap()
        .build();

    assert_eq!(config.keys().collect::<Vec<_>>(), vec!["a", "b"]);

    let map: BTreeMap<String, ConfigValue> = config.to_map();
    assert!(map.contains_key("a"));
    assert_eq!(map.len(), 2);

    let into: BTreeMap<String, ConfigValue> = config.into_map();
    assert_eq!(into.len(), 2);
}

#[test]
fn index_reaches_nested_paths() {
    let config = builder()
        .add_json(r#"{"server": {"port": 8080}}"#)
        .unwrap()
        .build();
    assert_eq!(config["server.port"].as_int(), Some(8080));
    assert_eq!(config["server"].as_table().map(BTreeMap::len), Some(1));
}

#[test]
fn sources_render_as_lower_case_names() {
    assert_eq!(Source::Defaults.as_str(), "defaults");
    assert_eq!(Source::Json.as_str(), "json");
    assert_eq!(Source::Toml.as_str(), "toml");
    assert_eq!(Source::Ini.as_str(), "ini");
    assert_eq!(Source::DotEnv.as_str(), "dotenv");
    assert_eq!(Source::Env.as_str(), "env");
    assert_eq!(Source::Explicit.as_str(), "explicit");
    assert_eq!(Source::Env.to_string(), "env");
}

#[test]
fn empty_configs_have_no_keys() {
    let config = Config::empty();
    assert!(config.keys().next().is_none());
    assert!(config.get("anything").is_none());
    assert!(config.source_of("anything").is_none());
    assert_eq!(config.to_map().len(), 0);

    let built = builder().into_config();
    assert!(built.keys().next().is_none());
}

// ------------------------------------------------------------ regressions --

#[test]
fn defaults_do_not_overwrite_existing_non_table_ancestors() {
    for raw in [
        r#"{"db": "sqlite::memory:"}"#,
        r#"{"db": [1, 2]}"#,
        r#"{"db": null}"#,
    ] {
        let base = builder().add_json(raw).unwrap().build();
        let expected_map = base.to_map();

        let with_def = base.with_default("db.host", "localhost");
        assert_eq!(with_def.to_map(), expected_map);
        assert_eq!(with_def.source_of("db"), Some(&Source::Json));
        assert!(with_def.get("db.host").is_none());
        assert!(with_def.source_of("db.host").is_none());

        let set_def = builder()
            .add_json(raw)
            .unwrap()
            .set_default("db.host", "localhost")
            .build();
        assert_eq!(set_def.to_map(), expected_map);
        assert_eq!(set_def.source_of("db"), Some(&Source::Json));
        assert!(set_def.get("db.host").is_none());
        assert!(set_def.source_of("db.host").is_none());
    }
}

#[test]
fn set_and_defaults_with_table_merge_leaf_wise_and_track_sources() {
    let mut table = BTreeMap::new();
    table.insert("port".to_string(), ConfigValue::from(6543i64));
    table.insert("tls".to_string(), ConfigValue::from(true));

    let mut nested = BTreeMap::new();
    nested.insert("min".to_string(), ConfigValue::from(2i64));
    table.insert("pool".to_string(), ConfigValue::Table(nested));

    let config = builder()
        .add_toml("[db]\nhost = \"localhost\"\nport = 5432\n[db.pool]\nmax = 10\n")
        .unwrap()
        .set("db", ConfigValue::Table(table))
        .build();

    assert_eq!(config.require_str("db.host").unwrap(), "localhost");
    assert_eq!(config.source_of("db.host"), Some(&Source::Toml));
    assert_eq!(config.require_int("db.port").unwrap(), 6543);
    assert_eq!(config.source_of("db.port"), Some(&Source::Explicit));
    assert!(config.require_bool("db.tls").unwrap());
    assert_eq!(config.source_of("db.tls"), Some(&Source::Explicit));
    assert_eq!(config.require_int("db.pool.max").unwrap(), 10);
    assert_eq!(config.source_of("db.pool.max"), Some(&Source::Toml));
    assert_eq!(config.require_int("db.pool.min").unwrap(), 2);
    assert_eq!(config.source_of("db.pool.min"), Some(&Source::Explicit));

    // Default table merge preserves existing leaves and sources while filling gaps.
    let mut default_table = BTreeMap::new();
    default_table.insert("host".to_string(), ConfigValue::from("shadowed"));
    default_table.insert("name".to_string(), ConfigValue::from("app"));
    let mut default_pool = BTreeMap::new();
    default_pool.insert("max".to_string(), ConfigValue::from(99i64));
    default_pool.insert("idle".to_string(), ConfigValue::from(30i64));
    default_table.insert("pool".to_string(), ConfigValue::Table(default_pool));

    let with_defaults = builder()
        .add_toml("[db]\nhost = \"primary\"\n[db.pool]\nmax = 10\n")
        .unwrap()
        .set_default("db", ConfigValue::Table(default_table.clone()))
        .build();

    assert_eq!(with_defaults.require_str("db.host").unwrap(), "primary");
    assert_eq!(with_defaults.source_of("db.host"), Some(&Source::Toml));
    assert_eq!(with_defaults.require_str("db.name").unwrap(), "app");
    assert_eq!(with_defaults.source_of("db.name"), Some(&Source::Defaults));
    assert_eq!(with_defaults.require_int("db.pool.max").unwrap(), 10);
    assert_eq!(with_defaults.source_of("db.pool.max"), Some(&Source::Toml));
    assert_eq!(with_defaults.require_int("db.pool.idle").unwrap(), 30);
    assert_eq!(
        with_defaults.source_of("db.pool.idle"),
        Some(&Source::Defaults)
    );

    let via_with_default = builder()
        .add_toml("[db]\nhost = \"primary\"\n[db.pool]\nmax = 10\n")
        .unwrap()
        .build()
        .with_default("db", ConfigValue::Table(default_table));

    assert_eq!(via_with_default.to_map(), with_defaults.to_map());
    assert_eq!(via_with_default.source_of("db.host"), Some(&Source::Toml));
    assert_eq!(
        via_with_default.source_of("db.name"),
        Some(&Source::Defaults)
    );
    assert_eq!(
        via_with_default.source_of("db.pool.idle"),
        Some(&Source::Defaults)
    );
}

#[test]
fn literal_dotted_keys_do_not_collide_in_sources_or_purge() {
    let config = builder()
        .add_toml("[a]\nb = 2\n")
        .unwrap()
        .add_json(r#"{"a.b": 1, "": "empty"}"#)
        .unwrap()
        .build();

    assert_eq!(config.get_segments(&["a.b"]), Some(&ConfigValue::Int(1)));
    assert_eq!(config.get_segments(&["a", "b"]), Some(&ConfigValue::Int(2)));
    assert_eq!(
        config.get_segments(&[""]),
        Some(&ConfigValue::String("empty".to_string()))
    );
    assert_eq!(config.get("a.b"), Some(&ConfigValue::Int(2)));
    assert_eq!(config.source_of("a.b"), Some(&Source::Toml));

    // Purging `["a"]` removes `["a", "b"]` from sources, without affecting `["a.b"]`.
    let purged = builder()
        .add_json(r#"{"a": {"b.c": 1}}"#)
        .unwrap()
        .add_toml("[a.b]\nc = 2\n")
        .unwrap()
        .set("a.b", "flat")
        .build();

    assert_eq!(
        purged.get_segments(&["a", "b.c"]),
        Some(&ConfigValue::Int(1))
    );
    assert_eq!(purged.require_str("a.b").unwrap(), "flat");
    assert_eq!(purged.source_of("a.b"), Some(&Source::Explicit));
    assert!(purged.get("a.b.c").is_none());
    assert!(purged.source_of("a.b.c").is_none());
}

#[test]
fn source_of_normalizes_dotted_paths() {
    let config = builder().add_toml("[a]\nb = 1\n").unwrap().build();

    assert_eq!(config.source_of("a.b"), Some(&Source::Toml));
    assert_eq!(config.source_of("a..b."), config.source_of("a.b"));
    assert_eq!(config.source_of(".a.b"), config.source_of("a.b"));
    assert_eq!(config.source_of(""), None);
    assert_eq!(config.source_of("..."), None);
}

#[test]
fn get_float_rejects_non_finite_strings() {
    let config = builder()
        .add_json(
            r#"{
                "nan": "NaN",
                "inf": "inf",
                "ninf": "-inf",
                "infinity": "Infinity",
                "ninfinity": "-Infinity",
                "finite": " 3.5 "
            }"#,
        )
        .unwrap()
        .build();

    assert_eq!(config.get_float("nan"), None);
    assert_eq!(config.get_float("inf"), None);
    assert_eq!(config.get_float("ninf"), None);
    assert_eq!(config.get_float("infinity"), None);
    assert_eq!(config.get_float("ninfinity"), None);
    assert_eq!(config.get_float("finite"), Some(3.5));
    assert!(config.require_float("nan").is_err());
    assert!(config.require_float("inf").is_err());
}

#[test]
fn builder_set_default_respects_precedence() {
    let config = builder()
        .set_default("host", "default-host")
        .set_default("port", 8080)
        .add_toml("port = 9090\n")
        .unwrap()
        .set_default("port", 1111)
        .set_default("tls.enabled", false)
        .build();

    assert_eq!(config.require_str("host").unwrap(), "default-host");
    assert_eq!(config.source_of("host"), Some(&Source::Defaults));
    assert_eq!(config.require_int("port").unwrap(), 9090);
    assert_eq!(config.source_of("port"), Some(&Source::Toml));
    assert!(!config.require_bool("tls.enabled").unwrap());
    assert_eq!(config.source_of("tls.enabled"), Some(&Source::Defaults));
}

#[test]
fn env_key_strips_at_most_one_separator_underscore() {
    let config = builder()
        .add_env_iter("APP", [("APP_PORT", "80"), ("APP__PORT", "443")])
        .build();

    assert_eq!(config.require_int("port").unwrap(), 80);
    assert_eq!(config.require_int("_port").unwrap(), 443);

    let underscored = builder()
        .add_env_iter("APP_", [("APP_PORT", "80"), ("APP__PORT", "443")])
        .build();
    assert_eq!(underscored.to_map(), config.to_map());
}

#[test]
fn deep_paths_and_values_do_not_overflow_stack() {
    let depth = 1_000;
    let deep_path = vec!["k"; depth].join(".");
    let config = builder().set(&deep_path, 42).build();
    assert_eq!(config.require_int(&deep_path).unwrap(), 42);
    drop(config);

    let mut nested = ConfigValue::from(1i64);
    for _ in 0..50_000 {
        let mut table = BTreeMap::new();
        table.insert("next".to_string(), nested);
        nested = ConfigValue::List(vec![ConfigValue::Table(table)]);
    }
    drop(nested);
}
