use std::ffi::OsString;

use akku::cli::{App, Arg, Command, ErrorKind, ValueSource};

#[test]
fn combined_short_flags_counts_and_attached_option_values() {
    let app = App::new("tool")
        .arg(Arg::flag("alpha").short('a'))
        .arg(Arg::flag("beta").short('b'))
        .arg(Arg::flag("gamma").short('c'))
        .arg(Arg::count("verbose").short('v').long("verbose"))
        .arg(Arg::option("port").short('p').long("port"));

    // -abc -vvv -p8080
    let m = app
        .parse_from_with_env(["tool", "-abc", "-vvv", "-p8080"], |_| None)
        .unwrap();
    assert!(m.get_flag("alpha"));
    assert!(m.get_flag("beta"));
    assert!(m.get_flag("gamma"));
    assert_eq!(m.get_count("verbose"), 3);
    assert_eq!(m.get_parsed::<u16>("port").unwrap(), Some(8080));

    // -abp9090 (combined flags ending in option with attached value)
    let m2 = app
        .parse_from_with_env(["tool", "-abp9090"], |_| None)
        .unwrap();
    assert!(m2.get_flag("alpha"));
    assert!(m2.get_flag("beta"));
    assert!(!m2.get_flag("gamma"));
    assert_eq!(m2.get_str("port"), Some("9090"));

    // --opt=val syntax
    let m3 = app
        .parse_from_with_env(["tool", "--port=443"], |_| None)
        .unwrap();
    assert_eq!(m3.get_parsed::<u16>("port").unwrap(), Some(443));
}

#[test]
fn double_dash_single_dash_positional_and_hyphen_values() {
    let cmd = Command::new("calc")
        .arg(
            Arg::option("offset")
                .short('o')
                .long("offset")
                .allow_hyphen_values(true),
        )
        .arg(Arg::positional("files").variadic(true));

    // Single "-" as positional, negative value for allow_hyphen_values, and "--" end-of-flags.
    let m = cmd
        .parse_from_with_env(
            ["calc", "-o", "-42", "-", "--", "--not-a-flag", "-x"],
            |_| None,
        )
        .unwrap();
    assert_eq!(m.get_parsed::<i32>("offset").unwrap(), Some(-42));
    assert_eq!(m.get_many("files"), Some(vec!["-", "--not-a-flag", "-x"]));

    // Without allow_hyphen_values, a flag-like token after an option is MissingValue.
    let strict = Command::new("strict")
        .arg(Arg::option("name").short('n'))
        .arg(Arg::flag("other").short('x'));
    let err = strict
        .parse_from_with_env(["strict", "-n", "-x"], |_| None)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::MissingValue);
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn nested_subcommands_and_global_flags_before_or_after() {
    let cli = || {
        Command::new("akku")
            .version("0.1.0")
            .about("Battery pack CLI")
            .subcommand_required(true)
            .arg(
                Arg::count("verbose")
                    .short('v')
                    .long("verbose")
                    .global(true),
            )
            .arg(
                Arg::option("config")
                    .short('c')
                    .long("config")
                    .global(true)
                    .default_value("akku.toml"),
            )
            .subcommand(
                Command::new("db")
                    .about("Database operations")
                    .subcommand_required(true)
                    .subcommand(
                        Command::new("migrate")
                            .about("Run migrations")
                            .arg(Arg::flag("dry-run").long("dry-run"))
                            .arg(Arg::option("steps").long("steps").multiple(true)),
                    ),
            )
    };

    // Global flags BEFORE subcommand.
    let m1 = cli()
        .parse_from_with_env(
            [
                "akku",
                "-vv",
                "--config",
                "prod.toml",
                "db",
                "migrate",
                "--dry-run",
            ],
            |_| None,
        )
        .unwrap();
    assert_eq!(m1.get_count("verbose"), 2);
    assert_eq!(m1.get_str("config"), Some("prod.toml"));
    let (s1, sub1) = m1.subcommand().unwrap();
    assert_eq!(s1, "db");
    let (s2, sub2) = sub1.subcommand().unwrap();
    assert_eq!(s2, "migrate");
    assert!(sub2.get_flag("dry-run"));
    assert_eq!(sub2.get_count("verbose"), 2);
    assert_eq!(sub2.get_str("config"), Some("prod.toml"));

    // Global flags AFTER nested subcommand propagate back up.
    let m2 = cli()
        .parse_from_with_env(
            [
                "akku",
                "db",
                "migrate",
                "-v",
                "--steps",
                "1",
                "--steps",
                "2",
                "--config=dev.toml",
            ],
            |_| None,
        )
        .unwrap();
    assert_eq!(m2.get_count("verbose"), 1);
    assert_eq!(m2.get_str("config"), Some("dev.toml"));
    let (_, db_m) = m2.subcommand().unwrap();
    let (_, mig_m) = db_m.subcommand().unwrap();
    assert_eq!(
        mig_m.get_many_parsed::<u32>("steps").unwrap(),
        Some(vec![1, 2])
    );

    // Missing required subcommand.
    let err = cli().parse_from_with_env(["akku"], |_| None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::MissingRequired);

    // Unknown subcommand.
    let err2 = cli()
        .parse_from_with_env(["akku", "nope"], |_| None)
        .unwrap_err();
    assert_eq!(err2.kind(), ErrorKind::UnknownSubcommand);
}

#[test]
fn env_and_default_value_sources_and_help_version_handling() {
    let cmd = Command::new("srv")
        .version("2.3.4")
        .about("HTTP server")
        .arg(
            Arg::option("host")
                .long("host")
                .env("APP_HOST")
                .default_value("127.0.0.1"),
        )
        .arg(
            Arg::option("port")
                .short('p')
                .long("port")
                .env("APP_PORT")
                .required(true),
        );

    // Env overrides default, CLI overrides env.
    let m = cmd
        .parse_from_with_env(["srv", "-p", "9000"], |k| match k {
            "APP_HOST" => Some("0.0.0.0".to_string()),
            "APP_PORT" => Some("3000".to_string()),
            _ => None,
        })
        .unwrap();
    assert_eq!(m.get_str("host"), Some("0.0.0.0"));
    assert_eq!(m.value_source("host"), Some(ValueSource::Env));
    assert_eq!(m.get_parsed::<u16>("port").unwrap(), Some(9000));
    assert_eq!(m.value_source("port"), Some(ValueSource::Cli));

    // Default used when env absent.
    let m2 = cmd
        .parse_from_with_env(["srv"], |k| (k == "APP_PORT").then(|| "4000".to_string()))
        .unwrap();
    assert_eq!(m2.get_str("host"), Some("127.0.0.1"));
    assert_eq!(m2.value_source("host"), Some(ValueSource::Default));
    assert_eq!(m2.get_parsed::<u16>("port").unwrap(), Some(4000));
    assert_eq!(m2.value_source("port"), Some(ValueSource::Env));

    // Help & Version flags.
    let help_err = cmd
        .parse_from_with_env(["srv", "--help"], |_| None)
        .unwrap_err();
    assert_eq!(help_err.kind(), ErrorKind::DisplayHelp);
    assert!(help_err.is_help_or_version());
    assert_eq!(help_err.exit_code(), 0);
    assert!(help_err.to_string().contains("HTTP server"));
    assert!(help_err.to_string().contains("[env: APP_HOST]"));

    let ver_err = cmd
        .parse_from_with_env(["srv", "-V"], |_| None)
        .unwrap_err();
    assert_eq!(ver_err.kind(), ErrorKind::DisplayVersion);
    assert_eq!(ver_err.exit_code(), 0);
    assert_eq!(ver_err.to_string(), "srv 2.3.4\n");

    // Without .version(...), -V is unknown.
    let no_ver = Command::new("nover");
    let err = no_ver
        .parse_from_with_env(["nover", "-V"], |_| None)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnknownArgument);

    // Invalid spec detection (duplicate short flag).
    let bad_spec = Command::new("bad")
        .arg(Arg::flag("one").short('x'))
        .arg(Arg::flag("two").short('x'));
    let spec_err = bad_spec.parse_from_with_env(["bad"], |_| None).unwrap_err();
    assert_eq!(spec_err.kind(), ErrorKind::InvalidSpec);

    // Non-UTF-8 check (on Unix platforms).
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let bad_os = OsString::from_vec(vec![0xff, 0xfe]);
        let err = cmd
            .parse_from_with_env([OsString::from("srv"), bad_os], |_| None)
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidUtf8);
    }
}
