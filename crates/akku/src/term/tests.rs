use std::ffi::OsStr;
use std::io::{self, Cursor};
use std::time::Duration;

use akku::term::{
    display_width, resolve_color, strip_ansi, Align, BorderStyle, Color, ColorMode, ProgressBar,
    ProgressStyle, Prompt, Style, Table,
};

#[test]
fn resolve_color_follows_no_color_org_spec() {
    // Always / Never ignore TTY & env.
    assert!(resolve_color(
        ColorMode::Always,
        false,
        Some(OsStr::new("1")),
        Some("dumb")
    ));
    assert!(!resolve_color(
        ColorMode::Never,
        true,
        None,
        Some("xterm-256color")
    ));

    // Auto with TTY and no NO_COLOR -> enabled.
    assert!(resolve_color(
        ColorMode::Auto,
        true,
        None,
        Some("xterm-256color")
    ));
    // Auto with empty NO_COLOR="" does NOT disable (no-color.org!).
    assert!(resolve_color(
        ColorMode::Auto,
        true,
        Some(OsStr::new("")),
        Some("xterm-256color")
    ));
    // Auto with non-empty NO_COLOR="1" disables.
    assert!(!resolve_color(
        ColorMode::Auto,
        true,
        Some(OsStr::new("1")),
        Some("xterm-256color")
    ));
    // Auto with TERM=dumb disables.
    assert!(!resolve_color(ColorMode::Auto, true, None, Some("dumb")));
    // Auto without TTY disables.
    assert!(!resolve_color(
        ColorMode::Auto,
        false,
        None,
        Some("xterm-256color")
    ));
}

#[test]
fn styling_ansi_stripping_and_unicode_display_width() {
    const ERR_STYLE: Style = Style::new().fg(Color::Rgb(255, 80, 0)).bold().underline();

    let painted = ERR_STYLE.paint("Error").when(true).to_string();
    assert!(painted.starts_with("\x1b["));
    assert!(painted.ends_with("\x1b[0m"));
    assert_eq!(strip_ansi(&painted), "Error");
    assert_eq!(display_width(&painted), 5);

    let plain = ERR_STYLE.paint_if("Error", false).to_string();
    assert_eq!(plain, "Error");

    // 256-color and background support.
    let badge = Style::new()
        .fg(Color::Ansi256(15))
        .bg(Color::Blue)
        .paint("INFO")
        .to_string();
    assert!(badge.contains("38;5;15"));
    assert!(badge.contains("44"));
    assert_eq!(strip_ansi(&badge), "INFO");

    // Unicode widths: ASCII = 1, CJK = 2, combining acute = 0, emoji = 2.
    assert_eq!(display_width("hello"), 5);
    assert_eq!(display_width("日本"), 4);
    assert_eq!(display_width("e\u{0301}"), 1); // 'e' + combining acute accent
    assert_eq!(display_width("🚀"), 2);
}

#[test]
fn table_rendering_across_borders_alignments_and_ansi_cells() {
    let ok = Style::new()
        .green()
        .bold()
        .paint("OK")
        .when(true)
        .to_string();
    let table = Table::new()
        .border(BorderStyle::Ascii)
        .headers(["Service", "Status", "Count"])
        .alignments([Align::Left, Align::Center, Align::Right])
        .row(["auth", &ok, "42"])
        .row(["billing", "WAIT", "7"]);

    let rendered = table.render();
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), 6);
    // Every line must have the exact same visible terminal width despite ANSI codes in row 1!
    let expected_w = display_width(lines[0]);
    for line in &lines {
        assert_eq!(display_width(line), expected_w, "misaligned line: {line:?}");
    }

    // Unicode, Rounded, and Compact border styles render cleanly.
    let rounded = Table::new()
        .border(BorderStyle::Rounded)
        .headers(["A", "B"])
        .row(["1", "2"])
        .render();
    assert!(rounded.starts_with('╭'));
    assert!(rounded.trim_end().ends_with('╯'));

    let compact = Table::new()
        .border(BorderStyle::Compact)
        .headers(["Key", "Val"])
        .row(["k", "v"])
        .render();
    assert!(compact.contains("───"));
    assert!(!compact.contains('│'));
}

#[test]
fn progress_bar_clamping_zero_total_and_rendering() {
    let mut pb = ProgressBar::new(100)
        .with_width(10)
        .with_style(ProgressStyle::Ascii)
        .with_message("downloading")
        .with_carriage_return(false);

    assert_eq!(pb.render(), "[----------]   0% (0/100) downloading");
    pb.inc(50);
    assert_eq!(pb.position(), 50);
    assert!(!pb.is_finished());
    assert_eq!(
        pb.render_with_elapsed(Duration::from_millis(1500)),
        "[#####-----]  50% (50/100) [1.5s] downloading"
    );

    // Clamping past total.
    pb.set(500);
    assert_eq!(pb.position(), 100);
    assert!(pb.is_finished());

    let mut out = Vec::new();
    pb.finish_to(&mut out).unwrap();
    let finished = String::from_utf8(out).unwrap();
    assert!(finished.ends_with('\n'));
    assert!(finished.contains("100%"));

    // Guard total == 0 (no divide by zero!).
    let zero_pb = ProgressBar::new(0)
        .with_width(4)
        .with_style(ProgressStyle::Ascii);
    assert!(zero_pb.is_finished());
    assert_eq!(zero_pb.render(), "[####] 100% (0/0)");
}

#[test]
fn prompt_text_confirm_select_reprompt_and_eof() {
    // Text + default + confirm (with invalid reprompt) + select (with invalid reprompt).
    let input = b"Alice\n\nmaybe\nyes\n99\n2\n";
    let mut out = Vec::new();
    let mut prompt = Prompt::new(Cursor::new(&input[..]), &mut out);

    assert_eq!(prompt.text("Name").unwrap(), "Alice");
    assert_eq!(prompt.text_with_default("Role", "dev").unwrap(), "dev");
    assert!(prompt.confirm("Proceed", false).unwrap());
    assert_eq!(
        prompt.select("Env", &["dev", "staging", "prod"]).unwrap(),
        1
    );

    let transcript = String::from_utf8(out).unwrap();
    assert!(transcript.contains("Please answer 'y' or 'n'"));
    assert!(transcript.contains("Invalid selection"));

    // Empty options -> InvalidInput.
    let mut out2 = Vec::new();
    let mut p2 = Prompt::new(Cursor::new(&b"1\n"[..]), &mut out2);
    let err = p2.select("Empty", &[]).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

    // Immediate EOF -> UnexpectedEof.
    let mut out3 = Vec::new();
    let mut p3 = Prompt::new(Cursor::new(&b""[..]), &mut out3);
    assert_eq!(
        p3.text("Name").unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert_eq!(
        p3.confirm("Ok", true).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert_eq!(
        p3.select("Pick", &["a"]).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}
