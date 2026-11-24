//! Integration tests for `akku::process`.
//!
//! The main suite drives POSIX tools (`sh`, `cat`, `tr`, `sort`, ...) and
//! runs on unix; the `windows` module covers the same seams with `cmd /C`.

#[cfg(unix)]
use std::io::ErrorKind;

#[cfg(unix)]
use akku::process::{Cmd, CmdError, CmdOutput, Pipeline};

/// A shell one-liner as a command.
#[cfg(unix)]
fn sh(script: &str) -> Cmd {
    Cmd::new("sh").args(["-c", script])
}

#[cfg(unix)]
#[test]
fn cmd_captures_stdout_and_stderr() {
    let out = sh("echo out; echo err 1>&2").run().unwrap();
    assert!(out.success());
    assert_eq!(out.stdout_str().unwrap(), "out\n");
    assert_eq!(out.stderr_str().unwrap(), "err\n");
    assert_eq!(out.stdout_lossy(), "out\n");
    assert_eq!(out.stderr_lossy(), "err\n");
    assert!(out.ensure_success().is_ok());
}

#[cfg(unix)]
#[test]
fn cmd_passes_environment_configuration_through() {
    let one = sh("printf %s \"$AKKU_PROC_ENV\"")
        .env("AKKU_PROC_ENV", "env-value")
        .run()
        .unwrap();
    assert_eq!(one.stdout_str().unwrap(), "env-value");

    let many = sh("printf %s \"$A$B\"")
        .env("A", "a")
        .envs([("B", "b"), ("C", "c")])
        .run()
        .unwrap();
    assert_eq!(many.stdout_str().unwrap(), "ab");

    let gone = sh("if [ -n \"$B\" ]; then echo set; else echo unset; fi")
        .env("B", "x")
        .env_remove("B")
        .run()
        .unwrap();
    assert_eq!(gone.stdout_lossy(), "unset\n");
}

#[cfg(unix)]
#[test]
fn cmd_can_clear_then_set_the_child_environment() {
    let out = sh("printf %s \"$ONLY\"")
        .env_clear()
        .env("ONLY", "value")
        .run()
        .unwrap();
    assert_eq!(out.stdout_str().unwrap(), "value");
}

#[cfg(unix)]
#[test]
fn cmd_runs_in_the_configured_directory() {
    use akku::fs::TempDir;

    let dir = TempDir::new("cmd-dir").unwrap();
    let out = sh("pwd").dir(dir.as_path()).run().unwrap();
    let expected = akku::fs::canonicalize(dir.as_path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(out.stdout_lossy().trim(), expected);
}

#[cfg(unix)]
#[test]
fn cmd_feeds_the_stdin_payload() {
    let out = Cmd::new("cat")
        .stdin_bytes(b"hello pipe".as_slice())
        .run()
        .unwrap();
    assert_eq!(out.stdout_str().unwrap(), "hello pipe");

    let big = vec![b'z'; 100_000];
    let out = Cmd::new("wc").arg("-c").stdin_bytes(big).run().unwrap();
    let printed = out.stdout_lossy();
    assert_eq!(printed.trim(), "100000");
}

#[cfg(unix)]
#[test]
fn cmd_reports_failing_statuses() {
    let out = sh("echo before; exit 3").run().unwrap();
    assert!(!out.success());
    assert_eq!(out.stdout_lossy(), "before\n");

    let error = out.ensure_success().unwrap_err();
    assert_eq!(error.program, "sh");
    assert_eq!(error.output.status.to_string(), "exit status: 3");
    assert_eq!(error.to_string(), "command `sh` failed: exit status: 3");

    let ok = CmdOutput::new(out.status, Vec::new(), Vec::new());
    assert!(!ok.success());
}

#[cfg(unix)]
#[test]
fn cmd_error_display_truncates_stderr_on_utf8_boundaries() {
    let failed = sh("exit 7").run().unwrap();
    // 4095 ASCII bytes, a two-byte é starting at byte 4095, more payload.
    let mut stderr = vec![b'x'; 4098];
    stderr[4095..4097].copy_from_slice("é".as_bytes());
    stderr.extend_from_slice(b"tail");

    let error = CmdError {
        program: "truncated".to_owned(),
        output: CmdOutput::new(failed.status, Vec::new(), stderr),
    };
    let text = error.to_string();
    assert!(text.starts_with("command `truncated` failed: exit status: 7"));
    assert!(text.contains("stderr: "));
    assert!(text.ends_with("..."));
    // 4095 payload bytes plus the fixed display text.
    assert!(text.len() < 4095 + 200);
}

#[cfg(unix)]
#[test]
fn cmd_error_display_preserves_ascii_after_invalid_utf8() {
    let failed = sh("exit 9").run().unwrap();

    // Short stderr: invalid UTF-8 bytes followed by ASCII text must remain intact.
    let mut short_stderr = vec![0xFF, 0xFE];
    short_stderr.extend_from_slice(b"fatal error");
    let short_err = CmdError {
        program: "lossy-short".to_owned(),
        output: CmdOutput::new(failed.status, Vec::new(), short_stderr),
    };
    let short_text = short_err.to_string();
    assert!(
        short_text.contains("\u{FFFD}\u{FFFD}fatal error"),
        "{short_text}"
    );
    assert!(!short_text.ends_with("..."), "{short_text}");

    // Long stderr (> 4096 bytes): early invalid UTF-8 must not wipe out trailing ASCII up to limit.
    let mut long_stderr = vec![0xFF, 0x80];
    long_stderr.extend_from_slice(&vec![b'a'; 5000]);
    let long_err = CmdError {
        program: "lossy-long".to_owned(),
        output: CmdOutput::new(failed.status, Vec::new(), long_stderr),
    };
    let long_text = long_err.to_string();
    assert!(long_text.contains("\u{FFFD}\u{FFFD}aaaa"), "{long_text}");
    assert!(long_text.ends_with("..."), "{long_text}");
    assert!(long_text.len() > 4000, "len={}", long_text.len());
}

#[cfg(unix)]
#[test]
fn cmd_status_runs_with_inherited_streams() {
    assert!(Cmd::new("true").status().unwrap().success());
    assert!(!Cmd::new("false").status().unwrap().success());
}

#[cfg(unix)]
#[test]
fn cmd_exposes_program_and_arguments() {
    let cmd = Cmd::new("echo").args(["one", "two"]);
    assert_eq!(cmd.program(), "echo");
    assert_eq!(cmd.command().get_program(), "echo");
    assert_eq!(cmd.command().get_args().count(), 2);
    let out = cmd.run().unwrap();
    assert_eq!(out.stdout_str().unwrap(), "one two\n");
}

#[cfg(unix)]
#[test]
fn command_reexports_are_usable() {
    // The std re-exports compile and behave through akku::process.
    let status = akku::process::Command::new("true").status().unwrap();
    assert!(status.success());
    assert_ne!(akku::process::id(), 0);
}

#[cfg(unix)]
#[test]
fn pipeline_sorts_through_two_stages() {
    let out = Pipeline::new()
        .cmd(Cmd::new("printf").arg("c\na\nb\n"))
        .cmd(Cmd::new("sort"))
        .run()
        .unwrap();
    assert!(out.success());
    assert_eq!(out.stdout_str().unwrap(), "a\nb\nc\n");
    assert!(out.all_success());
    assert!(out.ensure_all_success().is_ok());
    assert!(out.ensure_success().is_ok());
}

#[cfg(unix)]
#[test]
fn pipeline_feeds_the_first_stage_stdin() {
    let out = Pipeline::new()
        .cmd(
            Cmd::new("tr")
                .args(["a-z", "A-Z"])
                .stdin_bytes("hello pipe"),
        )
        .run()
        .unwrap();
    assert_eq!(out.stdout_str().unwrap(), "HELLO PIPE");
}

#[cfg(unix)]
#[test]
fn pipeline_collects_stderr_per_stage_in_order() {
    // Each stage writes stderr before stdout: stage two never reads its
    // stdin, so stage one may be killed by SIGPIPE on its stdout write.
    let out = Pipeline::new()
        .cmd(sh("echo one-err 1>&2; echo one-out"))
        .cmd(sh("echo two-err 1>&2; echo two-out"))
        .run()
        .unwrap();
    assert_eq!(
        out.stderr,
        vec![b"one-err\n".to_vec(), b"two-err\n".to_vec()]
    );
    assert_eq!(
        out.stderr_lossy(),
        vec!["one-err\n".to_owned(), "two-err\n".to_owned()]
    );
    assert_eq!(out.stdout_lossy(), "two-out\n");
    assert!(out.stderr_str().iter().all(Result::is_ok));
}

#[cfg(unix)]
#[test]
fn pipeline_reports_early_stage_failures() {
    let out = Pipeline::new()
        .cmd(sh("echo boom 1>&2; exit 2"))
        .cmd(Cmd::new("cat"))
        .run()
        .unwrap();
    // `success()` reflects the last stage (`cat`), which succeeded.
    assert!(out.success());
    assert_eq!(out.statuses.len(), 2);
    assert!(!out.statuses[0].success());

    let error = out.ensure_all_success().unwrap_err();
    assert_eq!(error.program, "sh");
    assert_eq!(error.output.stderr, b"boom\n".to_vec());
    assert!(out.ensure_success().is_ok());
}

#[cfg(unix)]
#[test]
fn pipeline_run_is_reusable() {
    let pipeline = Pipeline::new()
        .cmd(Cmd::new("printf").arg("x\ny\n"))
        .cmd(Cmd::new("sort"));
    let first = pipeline.run().unwrap();
    let second = pipeline.run().unwrap();
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stderr, second.stderr);
    assert_eq!(first.statuses.len(), second.statuses.len());
    assert!(first.all_success() && second.all_success());
}

#[cfg(unix)]
#[test]
fn pipeline_passes_bounded_streams_cleanly() {
    let out = Pipeline::new()
        .cmd(Cmd::new("yes"))
        .cmd(Cmd::new("head").args(["-c", "1000"]))
        .run()
        .unwrap();
    assert_eq!(out.stdout.len(), 1000);
    assert_eq!(out.statuses.len(), 2);
    // `head` read its 1000 bytes and terminated; `yes` died like it would in
    // any shell pipeline, so only the consumer is checked for success.
    assert!(out.statuses[1].success());
}

#[cfg(unix)]
#[test]
fn pipeline_rejects_empty_and_invalid_configs() {
    let empty = Pipeline::new().run().unwrap_err();
    assert_eq!(empty.kind(), ErrorKind::InvalidInput);
    assert!(Pipeline::new().is_empty());

    let later = Pipeline::new().cmd(Cmd::new("echo").arg("hi"));
    assert_eq!(later.len(), 1);
    let err = later
        .cmd(Cmd::new("cat").stdin_bytes(b"x"))
        .run()
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[cfg(unix)]
#[test]
fn pipeline_runs_stages_in_configured_directories() {
    use akku::fs::TempDir;

    let dir = TempDir::new("pipe-dir").unwrap();
    let out = Pipeline::new()
        .cmd(Cmd::new("pwd").dir(dir.as_path()))
        .run()
        .unwrap();
    let expected = akku::fs::canonicalize(dir.as_path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(out.stdout_lossy().trim(), expected);
}

#[cfg(unix)]
#[test]
fn pipeline_propagates_environment_configuration() {
    let out = Pipeline::new()
        .cmd(
            Cmd::new("sh")
                .args(["-c", "printf %s \"$P_VAL\""])
                .env("P_VAL", "carried"),
        )
        .run()
        .unwrap();
    assert_eq!(out.stdout_str().unwrap(), "carried");
}

/// The same seams on Windows, driven by `cmd.exe` builtins and `sort.exe`.
#[cfg(windows)]
mod windows {
    use akku::fs::TempDir;
    use akku::process::{Cmd, Pipeline};

    /// A `cmd.exe` one-liner as a command. `script` must not contain `"`:
    /// std quotes arguments with MSVC rules, which `cmd /C` does not parse.
    fn cmd(script: &str) -> Cmd {
        Cmd::new("cmd").args(["/C", script])
    }

    /// Output lines without their `\r\n` / `\n` terminators.
    fn lines(text: &str) -> Vec<String> {
        text.lines()
            .map(|line| line.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn cmd_captures_stdout_and_stderr() {
        let out = cmd("echo out&1>&2 echo err").run().unwrap();
        assert!(out.success());
        assert_eq!(out.stdout_lossy().trim_end(), "out");
        assert_eq!(out.stderr_lossy().trim_end(), "err");
        assert!(out.ensure_success().is_ok());
    }

    #[test]
    fn cmd_passes_environment_configuration_through() {
        let one = cmd("echo %AKKU_PROC_ENV%")
            .env("AKKU_PROC_ENV", "env-value")
            .run()
            .unwrap();
        assert_eq!(one.stdout_lossy().trim_end(), "env-value");

        let gone = cmd("if defined AKKU_PROC_GONE (echo set) else (echo unset)")
            .env("AKKU_PROC_GONE", "x")
            .env_remove("AKKU_PROC_GONE")
            .run()
            .unwrap();
        assert_eq!(gone.stdout_lossy().trim_end(), "unset");

        let only = cmd("echo %ONLY%")
            .env_clear()
            .env("ONLY", "value")
            .run()
            .unwrap();
        assert_eq!(only.stdout_lossy().trim_end(), "value");
    }

    #[test]
    fn cmd_runs_in_the_configured_directory() {
        let dir = TempDir::new("cmd-dir").unwrap();
        let out = cmd("cd").dir(dir.as_path()).run().unwrap();
        let printed = out.stdout_lossy().trim_end().to_lowercase();
        let given = dir.as_path().to_string_lossy().to_lowercase();
        let canonical = akku::fs::canonicalize(dir.as_path())
            .unwrap()
            .to_string_lossy()
            .to_lowercase();
        let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
        assert!(
            printed == given || printed == canonical,
            "cd printed {printed:?}, expected {given:?} or {canonical:?}"
        );
    }

    #[test]
    fn cmd_feeds_the_stdin_payload_and_reports_failures() {
        let sorted = Cmd::new("sort").stdin_bytes("b\na\n").run().unwrap();
        assert_eq!(lines(&sorted.stdout_lossy()), ["a", "b"]);

        let out = cmd("echo before&exit 3").run().unwrap();
        assert!(!out.success());
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout_lossy().trim_end(), "before");
        let error = out.ensure_success().unwrap_err();
        assert_eq!(error.program, "cmd");
    }

    #[test]
    fn pipeline_sorts_and_collects_stderr_per_stage() {
        let out = Pipeline::new()
            .cmd(cmd("echo c&echo a&echo b"))
            .cmd(Cmd::new("sort"))
            .run()
            .unwrap();
        assert!(out.all_success());
        assert_eq!(lines(&out.stdout_lossy()), ["a", "b", "c"]);

        let out = Pipeline::new()
            .cmd(cmd("1>&2 echo one-err&echo one-out"))
            .cmd(cmd("1>&2 echo two-err&echo two-out"))
            .run()
            .unwrap();
        let stderr: Vec<String> = out
            .stderr_lossy()
            .iter()
            .map(|text| text.trim_end().to_owned())
            .collect();
        assert_eq!(stderr, ["one-err", "two-err"]);
        assert_eq!(out.stdout_lossy().trim_end(), "two-out");
    }

    #[test]
    fn pipeline_reports_early_stage_failures() {
        let out = Pipeline::new()
            .cmd(cmd("1>&2 echo boom&exit 2"))
            .cmd(Cmd::new("sort"))
            .run()
            .unwrap();
        assert!(out.success());
        assert_eq!(out.statuses.len(), 2);
        assert!(!out.statuses[0].success());
        let error = out.ensure_all_success().unwrap_err();
        assert_eq!(error.program, "cmd");
        assert_eq!(error.output.stderr_lossy().trim_end(), "boom");
    }
}
