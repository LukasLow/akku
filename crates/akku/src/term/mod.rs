//! Terminal styling, ANSI-aware width measurement, tables, progress bars,
//! TTY detection, and interactive prompts.
//!
//! # Examples
//!
//! ```
//! use akku::term::{Align, BorderStyle, Style, Table, display_width, strip_ansi};
//!
//! let styled = Style::new().green().bold().paint("OK").when(true).to_string();
//! assert_eq!(strip_ansi(&styled), "OK");
//! assert_eq!(display_width(&styled), 2);
//!
//! let table = Table::new()
//!     .border(BorderStyle::Ascii)
//!     .headers(["Name", "Status"])
//!     .align(1, Align::Right)
//!     .row(["api", "ready"]);
//! assert!(table.render().contains("ready"));
//! ```

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufRead, BufReader, IsTerminal, Stdin, Stdout, Write};
use std::time::Duration;

// ---------------------------------------------------------------------------
// TTY & color detection (no-color.org compliant)
// ---------------------------------------------------------------------------

/// Standard terminal output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stream {
    /// Standard output (`stdout`).
    Stdout,
    /// Standard error (`stderr`).
    Stderr,
}

/// Policy controlling whether ANSI color escape codes are emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ColorMode {
    /// Emit color when writing to a TTY unless `NO_COLOR` is non-empty or `TERM=dumb`.
    #[default]
    Auto,
    /// Always emit ANSI color escape codes.
    Always,
    /// Never emit ANSI color escape codes.
    Never,
}

/// Returns `true` if `stream` is attached to an interactive terminal (TTY).
pub fn is_tty(stream: Stream) -> bool {
    match stream {
        Stream::Stdout => io::stdout().is_terminal(),
        Stream::Stderr => io::stderr().is_terminal(),
    }
}

/// Pure decision function for color enablement respecting [no-color.org](https://no-color.org/).
///
/// In [`ColorMode::Auto`], a non-empty `NO_COLOR` disables color, whereas an
/// empty `NO_COLOR=""` does **not** disable color. `TERM=dumb` also disables color.
pub fn resolve_color(
    mode: ColorMode,
    is_tty: bool,
    no_color: Option<&OsStr>,
    term: Option<&str>,
) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            if let Some(val) = no_color {
                if !val.is_empty() {
                    return false;
                }
            }
            if matches!(term, Some("dumb")) {
                return false;
            }
            is_tty
        }
    }
}

/// Returns `true` if ANSI color should be emitted on `stream` in [`ColorMode::Auto`].
pub fn should_color(stream: Stream) -> bool {
    let no_color = std::env::var_os("NO_COLOR");
    let term = std::env::var("TERM").ok();
    resolve_color(
        ColorMode::Auto,
        is_tty(stream),
        no_color.as_deref(),
        term.as_deref(),
    )
}

// ---------------------------------------------------------------------------
// ANSI stripping & Unicode display width
// ---------------------------------------------------------------------------

/// Strips ANSI CSI, OSC, and 2-byte escape sequences from `input`.
pub fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                // CSI: ESC [ ... final byte 0x40..=0x7E
                b'[' => {
                    i += 1;
                    while i < bytes.len() {
                        let b = bytes[i];
                        i += 1;
                        if (0x40..=0x7E).contains(&b) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... BEL (0x07) or ST (ESC \)
                b']' => {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                // 2-byte Fe/Fp/Fs sequence
                0x40..=0x7E => {
                    i += 1;
                }
                _ => {}
            }
        } else {
            // Safe UTF-8 char boundary copy.
            let ch = input[i..].chars().next().expect("valid UTF-8 char");
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    out
}

fn char_display_width(ch: char) -> usize {
    let cp = ch as u32;
    // C0, DEL, C1 controls
    if cp <= 0x001F || (0x007F..=0x009F).contains(&cp) {
        return 0;
    }
    // Zero-width formatters, variation selectors, and common combining mark blocks
    if matches!(
        cp,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0xFEFF
            | 0xE0100..=0xE01EF
    ) {
        return 0;
    }
    // East Asian Wide / Fullwidth CJK, Hangul, Kana, and Emoji blocks
    if matches!(
        cp,
        0x1100..=0x115F
            | 0x2329..=0x232A
            | 0x2600..=0x27BF
            | 0x2E80..=0x303E
            | 0x3040..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD
    ) {
        return 2;
    }
    1
}

/// Computes the terminal cell width of `input` after stripping ANSI escape codes.
pub fn display_width(input: &str) -> usize {
    strip_ansi(input).chars().map(char_display_width).sum()
}

// ---------------------------------------------------------------------------
// Colors & Styles
// ---------------------------------------------------------------------------

/// ANSI terminal color (standard 16-color, 256-color palette, or 24-bit RGB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// Standard black (30/40).
    Black,
    /// Standard red (31/41).
    Red,
    /// Standard green (32/42).
    Green,
    /// Standard yellow (33/43).
    Yellow,
    /// Standard blue (34/44).
    Blue,
    /// Standard magenta (35/45).
    Magenta,
    /// Standard cyan (36/46).
    Cyan,
    /// Standard white (37/47).
    White,
    /// Bright black / dark gray (90/100).
    BrightBlack,
    /// Bright red (91/101).
    BrightRed,
    /// Bright green (92/102).
    BrightGreen,
    /// Bright yellow (93/103).
    BrightYellow,
    /// Bright blue (94/104).
    BrightBlue,
    /// Bright magenta (95/105).
    BrightMagenta,
    /// Bright cyan (96/106).
    BrightCyan,
    /// Bright white (97/107).
    BrightWhite,
    /// 8-bit xterm palette index (`0..=255`).
    Ansi256(u8),
    /// 24-bit truecolor `(r, g, b)`.
    Rgb(u8, u8, u8),
}

impl Color {
    fn write_fg(self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Black => f.write_str("30"),
            Self::Red => f.write_str("31"),
            Self::Green => f.write_str("32"),
            Self::Yellow => f.write_str("33"),
            Self::Blue => f.write_str("34"),
            Self::Magenta => f.write_str("35"),
            Self::Cyan => f.write_str("36"),
            Self::White => f.write_str("37"),
            Self::BrightBlack => f.write_str("90"),
            Self::BrightRed => f.write_str("91"),
            Self::BrightGreen => f.write_str("92"),
            Self::BrightYellow => f.write_str("93"),
            Self::BrightBlue => f.write_str("94"),
            Self::BrightMagenta => f.write_str("95"),
            Self::BrightCyan => f.write_str("96"),
            Self::BrightWhite => f.write_str("97"),
            Self::Ansi256(n) => write!(f, "38;5;{n}"),
            Self::Rgb(r, g, b) => write!(f, "38;2;{r};{g};{b}"),
        }
    }

    fn write_bg(self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Black => f.write_str("40"),
            Self::Red => f.write_str("41"),
            Self::Green => f.write_str("42"),
            Self::Yellow => f.write_str("43"),
            Self::Blue => f.write_str("44"),
            Self::Magenta => f.write_str("45"),
            Self::Cyan => f.write_str("46"),
            Self::White => f.write_str("47"),
            Self::BrightBlack => f.write_str("100"),
            Self::BrightRed => f.write_str("101"),
            Self::BrightGreen => f.write_str("102"),
            Self::BrightYellow => f.write_str("103"),
            Self::BrightBlue => f.write_str("104"),
            Self::BrightMagenta => f.write_str("105"),
            Self::BrightCyan => f.write_str("106"),
            Self::BrightWhite => f.write_str("107"),
            Self::Ansi256(n) => write!(f, "48;5;{n}"),
            Self::Rgb(r, g, b) => write!(f, "48;2;{r};{g};{b}"),
        }
    }
}

/// Composable ANSI text style (`const fn` builder).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
}

impl Style {
    /// Creates an empty style with no colors or attributes.
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            strikethrough: false,
            inverse: false,
        }
    }

    /// Sets the foreground color.
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Sets the background color.
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Enables bold text.
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Enables dim / faint text.
    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    /// Enables italic text.
    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Enables underlined text.
    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Enables strikethrough text.
    pub const fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    /// Enables inverse video (swapped foreground/background).
    pub const fn inverse(mut self) -> Self {
        self.inverse = true;
        self
    }

    /// Shorthand for `.fg(Color::Red)`.
    pub const fn red(self) -> Self {
        self.fg(Color::Red)
    }

    /// Shorthand for `.fg(Color::Green)`.
    pub const fn green(self) -> Self {
        self.fg(Color::Green)
    }

    /// Shorthand for `.fg(Color::Yellow)`.
    pub const fn yellow(self) -> Self {
        self.fg(Color::Yellow)
    }

    /// Shorthand for `.fg(Color::Blue)`.
    pub const fn blue(self) -> Self {
        self.fg(Color::Blue)
    }

    /// Shorthand for `.fg(Color::Cyan)`.
    pub const fn cyan(self) -> Self {
        self.fg(Color::Cyan)
    }

    /// Shorthand for `.fg(Color::Magenta)`.
    pub const fn magenta(self) -> Self {
        self.fg(Color::Magenta)
    }

    /// Wraps `value` in a [`Styled`] value with ANSI styling enabled.
    pub const fn paint<T>(self, value: T) -> Styled<T> {
        Styled {
            value,
            style: self,
            enabled: true,
        }
    }

    /// Wraps `value` in a [`Styled`] value with ANSI styling conditionally enabled.
    pub const fn paint_if<T>(self, value: T, enabled: bool) -> Styled<T> {
        Styled {
            value,
            style: self,
            enabled,
        }
    }

    const fn is_empty(&self) -> bool {
        self.fg.is_none()
            && self.bg.is_none()
            && !self.bold
            && !self.dim
            && !self.italic
            && !self.underline
            && !self.strikethrough
            && !self.inverse
    }
}

/// A value paired with a [`Style`] that renders ANSI SGR codes via [`fmt::Display`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Styled<T> {
    value: T,
    style: Style,
    enabled: bool,
}

impl<T> Styled<T> {
    /// Conditionally enables or disables ANSI styling on this value.
    pub const fn when(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Enables styling only when [`should_color`] is true for `stream`.
    pub fn for_stream(mut self, stream: Stream) -> Self {
        self.enabled = should_color(stream);
        self
    }

    /// Returns the attached [`Style`].
    pub const fn style(&self) -> Style {
        self.style
    }

    /// Consumes the wrapper and returns the inner value.
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: fmt::Display> fmt::Display for Styled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.enabled || self.style.is_empty() {
            return self.value.fmt(f);
        }

        f.write_str("\x1b[")?;
        let mut sep = false;
        let mut push_sep = |f: &mut fmt::Formatter<'_>| -> fmt::Result {
            if sep {
                f.write_str(";")?;
            }
            sep = true;
            Ok(())
        };

        if self.style.bold {
            push_sep(f)?;
            f.write_str("1")?;
        }
        if self.style.dim {
            push_sep(f)?;
            f.write_str("2")?;
        }
        if self.style.italic {
            push_sep(f)?;
            f.write_str("3")?;
        }
        if self.style.underline {
            push_sep(f)?;
            f.write_str("4")?;
        }
        if self.style.inverse {
            push_sep(f)?;
            f.write_str("7")?;
        }
        if self.style.strikethrough {
            push_sep(f)?;
            f.write_str("9")?;
        }
        if let Some(fg) = self.style.fg {
            push_sep(f)?;
            fg.write_fg(f)?;
        }
        if let Some(bg) = self.style.bg {
            push_sep(f)?;
            bg.write_bg(f)?;
        }

        write!(f, "m{}\x1b[0m", self.value)
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// Horizontal alignment of a table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Align {
    /// Left-align cell text.
    #[default]
    Left,
    /// Right-align cell text.
    Right,
    /// Center cell text.
    Center,
}

/// Box-drawing border theme for [`Table`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BorderStyle {
    /// Plain ASCII borders (`+`, `-`, `|`).
    Ascii,
    /// Unicode light box-drawing characters (`┌`, `─`, `│`).
    #[default]
    Unicode,
    /// Unicode rounded corners (`╭`, `─`, `│`).
    Rounded,
    /// Borderless compact layout with a single horizontal rule under headers.
    Compact,
}

struct BoxChars {
    tl: char,
    tm: char,
    tr: char,
    ml: char,
    mm: char,
    mr: char,
    bl: char,
    bm: char,
    br: char,
    h: char,
    v: char,
}

impl BorderStyle {
    fn chars(self) -> Option<BoxChars> {
        match self {
            Self::Ascii => Some(BoxChars {
                tl: '+',
                tm: '+',
                tr: '+',
                ml: '+',
                mm: '+',
                mr: '+',
                bl: '+',
                bm: '+',
                br: '+',
                h: '-',
                v: '|',
            }),
            Self::Unicode => Some(BoxChars {
                tl: '┌',
                tm: '┬',
                tr: '┐',
                ml: '├',
                mm: '┼',
                mr: '┤',
                bl: '└',
                bm: '┴',
                br: '┘',
                h: '─',
                v: '│',
            }),
            Self::Rounded => Some(BoxChars {
                tl: '╭',
                tm: '┬',
                tr: '╮',
                ml: '├',
                mm: '┼',
                mr: '┤',
                bl: '╰',
                bm: '┴',
                br: '╯',
                h: '─',
                v: '│',
            }),
            Self::Compact => None,
        }
    }
}

/// An ANSI- and Unicode-width-aware terminal table renderer.
#[derive(Debug, Clone, Default)]
pub struct Table {
    border: BorderStyle,
    padding: usize,
    headers: Option<Vec<String>>,
    rows: Vec<Vec<String>>,
    alignments: Vec<Align>,
}

impl Table {
    /// Creates an empty `Table` with [`BorderStyle::Unicode`] and horizontal padding `1`.
    pub fn new() -> Self {
        Self {
            border: BorderStyle::Unicode,
            padding: 1,
            headers: None,
            rows: Vec::new(),
            alignments: Vec::new(),
        }
    }

    /// Sets the table's [`BorderStyle`].
    pub fn border(mut self, border: BorderStyle) -> Self {
        self.border = border;
        self
    }

    /// Sets the number of spaces padded on each side of a cell.
    pub fn padding(mut self, padding: usize) -> Self {
        self.padding = padding;
        self
    }

    /// Sets the header row.
    pub fn headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.headers = Some(headers.into_iter().map(Into::into).collect());
        self
    }

    /// Appends a single data row.
    pub fn row<I, S>(mut self, row: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows.push(row.into_iter().map(Into::into).collect());
        self
    }

    /// Appends multiple data rows.
    pub fn rows<R, I, S>(mut self, rows: R) -> Self
    where
        R: IntoIterator<Item = I>,
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for r in rows {
            self = self.row(r);
        }
        self
    }

    /// Sets the alignment for a single 0-based column index.
    pub fn align(mut self, col: usize, align: Align) -> Self {
        if self.alignments.len() <= col {
            self.alignments.resize(col + 1, Align::Left);
        }
        self.alignments[col] = align;
        self
    }

    /// Sets alignments for all columns from an iterator.
    pub fn alignments<I>(mut self, alignments: I) -> Self
    where
        I: IntoIterator<Item = Align>,
    {
        self.alignments = alignments.into_iter().collect();
        self
    }

    /// Renders the table to a [`String`].
    pub fn render(&self) -> String {
        let mut col_count = self.headers.as_ref().map_or(0, Vec::len);
        for row in &self.rows {
            col_count = col_count.max(row.len());
        }
        if col_count == 0 {
            return String::new();
        }

        let mut widths = vec![0usize; col_count];
        if let Some(ref hdrs) = self.headers {
            for (i, cell) in hdrs.iter().enumerate() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }

        let mut out = String::new();
        let pad = " ".repeat(self.padding);

        let write_cell = |out: &mut String, cell: &str, width: usize, align: Align| {
            let w = display_width(cell);
            let extra = width.saturating_sub(w);
            match align {
                Align::Left => {
                    out.push_str(cell);
                    for _ in 0..extra {
                        out.push(' ');
                    }
                }
                Align::Right => {
                    for _ in 0..extra {
                        out.push(' ');
                    }
                    out.push_str(cell);
                }
                Align::Center => {
                    let left = extra / 2;
                    let right = extra - left;
                    for _ in 0..left {
                        out.push(' ');
                    }
                    out.push_str(cell);
                    for _ in 0..right {
                        out.push(' ');
                    }
                }
            }
        };

        if let Some(bx) = self.border.chars() {
            let write_rule = |out: &mut String, left: char, mid: char, right: char| {
                out.push(left);
                for (i, &w) in widths.iter().enumerate() {
                    if i > 0 {
                        out.push(mid);
                    }
                    for _ in 0..(w + self.padding * 2) {
                        out.push(bx.h);
                    }
                }
                out.push(right);
                out.push('\n');
            };

            let write_row = |out: &mut String, cells: &[String]| {
                out.push(bx.v);
                for (i, &w) in widths.iter().enumerate() {
                    let cell = cells.get(i).map(String::as_str).unwrap_or("");
                    let align = self.alignments.get(i).copied().unwrap_or(Align::Left);
                    out.push_str(&pad);
                    write_cell(out, cell, w, align);
                    out.push_str(&pad);
                    out.push(bx.v);
                }
                out.push('\n');
            };

            write_rule(&mut out, bx.tl, bx.tm, bx.tr);
            if let Some(ref hdrs) = self.headers {
                write_row(&mut out, hdrs);
                write_rule(&mut out, bx.ml, bx.mm, bx.mr);
            }
            for row in &self.rows {
                write_row(&mut out, row);
            }
            write_rule(&mut out, bx.bl, bx.bm, bx.br);
        } else {
            let gap = " ".repeat(self.padding.max(1) * 2);
            let write_compact_row = |out: &mut String, cells: &[String]| {
                for (i, &w) in widths.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&gap);
                    }
                    let cell = cells.get(i).map(String::as_str).unwrap_or("");
                    let align = self.alignments.get(i).copied().unwrap_or(Align::Left);
                    write_cell(out, cell, w, align);
                }
                out.push('\n');
            };

            if let Some(ref hdrs) = self.headers {
                write_compact_row(&mut out, hdrs);
                for (i, &w) in widths.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&gap);
                    }
                    for _ in 0..w {
                        out.push('─');
                    }
                }
                out.push('\n');
            }
            for row in &self.rows {
                write_compact_row(&mut out, row);
            }
        }

        out
    }

    /// Writes the rendered table to `writer`.
    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(self.render().as_bytes())
    }
}

impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

// ---------------------------------------------------------------------------
// ProgressBar
// ---------------------------------------------------------------------------

/// Visual character set for [`ProgressBar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProgressStyle {
    /// ASCII bar (`[####----]`).
    Ascii,
    /// Unicode block bar (`[████░░░░]`).
    #[default]
    Unicode,
}

/// Deterministic progress bar supporting custom widths, messages, and elapsed rendering.
#[derive(Debug, Clone)]
pub struct ProgressBar {
    total: u64,
    pos: u64,
    width: usize,
    style: ProgressStyle,
    message: String,
    carriage_return: bool,
}

impl ProgressBar {
    /// Creates a new `ProgressBar` with the given `total` step count.
    pub fn new(total: u64) -> Self {
        Self {
            total,
            pos: 0,
            width: 30,
            style: ProgressStyle::Unicode,
            message: String::new(),
            carriage_return: true,
        }
    }

    /// Sets the visual bar width in characters.
    pub fn with_width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    /// Sets the bar character style ([`ProgressStyle::Ascii`] or [`ProgressStyle::Unicode`]).
    pub fn with_style(mut self, style: ProgressStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets an initial status message displayed alongside the bar.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// Controls whether [`Self::draw_to`] prefixes output with `\r` for in-place updates.
    pub fn with_carriage_return(mut self, enabled: bool) -> Self {
        self.carriage_return = enabled;
        self
    }

    /// Sets the current position, clamped to `total`.
    pub fn set(&mut self, pos: u64) {
        self.pos = pos.min(self.total);
    }

    /// Increments the current position by `delta`, clamped to `total`.
    pub fn inc(&mut self, delta: u64) {
        self.pos = self.pos.saturating_add(delta).min(self.total);
    }

    /// Updates the status message.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }

    /// Returns the current clamped position.
    pub const fn position(&self) -> u64 {
        self.pos
    }

    /// Returns the total step count.
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Returns `true` if `position >= total` (including when `total == 0`).
    pub const fn is_finished(&self) -> bool {
        self.pos >= self.total
    }

    /// Renders the progress bar as a single line without trailing newline.
    pub fn render(&self) -> String {
        self.render_inner(None)
    }

    /// Renders the progress bar including an explicit `elapsed` duration.
    pub fn render_with_elapsed(&self, elapsed: Duration) -> String {
        self.render_inner(Some(elapsed))
    }

    fn render_inner(&self, elapsed: Option<Duration>) -> String {
        let ratio = if self.total == 0 {
            1.0
        } else {
            (self.pos as f64 / self.total as f64).clamp(0.0, 1.0)
        };
        let filled = ((ratio * self.width as f64).round() as usize).min(self.width);
        let empty = self.width - filled;
        let pct = (ratio * 100.0).round() as u64;

        let (fill_ch, empty_ch) = match self.style {
            ProgressStyle::Ascii => ('#', '-'),
            ProgressStyle::Unicode => ('█', '░'),
        };

        let mut bar = String::with_capacity(self.width + 32);
        bar.push('[');
        for _ in 0..filled {
            bar.push(fill_ch);
        }
        for _ in 0..empty {
            bar.push(empty_ch);
        }
        bar.push(']');

        let mut out = format!("{bar} {pct:3}% ({}/{})", self.pos, self.total);
        if let Some(el) = elapsed {
            out.push_str(&format!(" [{:.1}s]", el.as_secs_f64()));
        }
        if !self.message.is_empty() {
            out.push(' ');
            out.push_str(&self.message);
        }
        out
    }

    /// Draws the current bar state to `writer`.
    pub fn draw_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        if self.carriage_return {
            writer.write_all(b"\r")?;
        }
        writer.write_all(self.render().as_bytes())?;
        writer.flush()
    }

    /// Draws the current bar state with an explicit `elapsed` duration to `writer`.
    pub fn draw_to_with_elapsed<W: Write>(
        &self,
        writer: &mut W,
        elapsed: Duration,
    ) -> io::Result<()> {
        if self.carriage_return {
            writer.write_all(b"\r")?;
        }
        writer.write_all(self.render_with_elapsed(elapsed).as_bytes())?;
        writer.flush()
    }

    /// Marks the bar finished (`position = total`), draws it, and emits a trailing newline.
    pub fn finish_to<W: Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.pos = self.total;
        self.draw_to(writer)?;
        writer.write_all(b"\n")?;
        writer.flush()
    }
}

// ---------------------------------------------------------------------------
// Interactive Prompts
// ---------------------------------------------------------------------------

/// Testable interactive prompt reading from `R` and writing prompts to `W`.
#[derive(Debug)]
pub struct Prompt<R, W> {
    reader: R,
    writer: W,
}

impl Prompt<BufReader<Stdin>, Stdout> {
    /// Creates a `Prompt` wired to process `stdin` and `stdout`.
    pub fn stdio() -> Self {
        Self {
            reader: BufReader::new(io::stdin()),
            writer: io::stdout(),
        }
    }
}

impl<R: BufRead, W: Write> Prompt<R, W> {
    /// Creates a `Prompt` using custom reader and writer streams (ideal for tests).
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    fn read_line_trimmed(&mut self) -> io::Result<String> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected end of input",
            ));
        }
        Ok(line.trim_end_matches(['\r', '\n']).trim().to_string())
    }

    /// Prompts for a single line of text, returning the trimmed response.
    pub fn text(&mut self, label: &str) -> io::Result<String> {
        write!(self.writer, "{label}: ")?;
        self.writer.flush()?;
        self.read_line_trimmed()
    }

    /// Prompts for a line of text, returning `default` if the user submits an empty line.
    pub fn text_with_default(&mut self, label: &str, default: &str) -> io::Result<String> {
        write!(self.writer, "{label} [{default}]: ")?;
        self.writer.flush()?;
        let line = self.read_line_trimmed()?;
        if line.is_empty() {
            Ok(default.to_string())
        } else {
            Ok(line)
        }
    }

    /// Prompts for a yes/no confirmation, reprompting until valid input or EOF.
    pub fn confirm(&mut self, label: &str, default: bool) -> io::Result<bool> {
        let hint = if default { "Y/n" } else { "y/N" };
        loop {
            write!(self.writer, "{label} [{hint}]: ")?;
            self.writer.flush()?;
            let line = self.read_line_trimmed()?;
            if line.is_empty() {
                return Ok(default);
            }
            match line.to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => {
                    writeln!(self.writer, "Please answer 'y' or 'n'.")?;
                }
            }
        }
    }

    /// Displays a 1-based numbered menu for `options`, reprompting on invalid input,
    /// and returns the selected **0-based** index.
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `options` is empty, and
    /// [`io::ErrorKind::UnexpectedEof`] on EOF.
    pub fn select(&mut self, label: &str, options: &[&str]) -> io::Result<usize> {
        if options.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "select requires at least one option",
            ));
        }
        writeln!(self.writer, "{label}")?;
        for (idx, opt) in options.iter().enumerate() {
            writeln!(self.writer, "  {}) {opt}", idx + 1)?;
        }
        loop {
            write!(self.writer, "Select [1-{}]: ", options.len())?;
            self.writer.flush()?;
            let line = self.read_line_trimmed()?;
            if let Ok(choice) = line.parse::<usize>() {
                if (1..=options.len()).contains(&choice) {
                    return Ok(choice - 1);
                }
            }
            writeln!(
                self.writer,
                "Invalid selection; enter a number between 1 and {}.",
                options.len()
            )?;
        }
    }
}
