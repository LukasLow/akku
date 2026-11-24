//! Linear-space Myers diff engine, unified diff formatting, and patch application.
//!
//! Available at both `akku::text::diff` and `akku::diff`.

use std::fmt;
use std::hash::{Hash, Hasher};

use crate::text::grapheme::graphemes;

/// A single edit operation produced by a diff algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffOp<T> {
    /// Element is identical in both `old` and `new`.
    Equal(T),
    /// Element was inserted in `new`.
    Insert(T),
    /// Element was deleted from `old`.
    Delete(T),
}

/// Bounds for diffing huge or adversarial inputs in bounded time and memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffOptions {
    /// Maximum Myers edit distance before falling back to a bounded `Delete` + `Insert`
    /// block over the unmatched middle slice. Default: `Some(4096)`.
    pub max_edit_distance: Option<usize>,
    /// Maximum number of Myers diagonal evaluations before falling back.
    /// Default: `Some(2_000_000)`.
    pub max_steps: Option<usize>,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffOptions {
    /// Default bounded diff options (`max_edit_distance = Some(4096)`, `max_steps = Some(2_000_000)`).
    pub const fn new() -> Self {
        Self {
            max_edit_distance: Some(4096),
            max_steps: Some(2_000_000),
        }
    }

    /// Unbounded linear-space Myers diff options.
    pub const fn unbounded() -> Self {
        Self {
            max_edit_distance: None,
            max_steps: None,
        }
    }
}

/// A single hunk in a unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk<T> {
    /// One-based starting line in `old` (or `0` when inserting at the very start of an empty file).
    pub old_start: usize,
    /// Number of lines from `old` covered by this hunk.
    pub old_count: usize,
    /// One-based starting line in `new` (or `0` when deleting down to an empty file).
    pub new_start: usize,
    /// Number of lines from `new` covered by this hunk.
    pub new_count: usize,
    /// Ordered edit operations inside this hunk.
    pub ops: Vec<DiffOp<T>>,
}

/// Error returned by [`apply_patch`] when a unified diff cannot be parsed or applied cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffError {
    /// One-based line number in the patch where the error occurred, if applicable.
    pub line: Option<usize>,
    /// Human-readable description of the patch failure.
    pub message: String,
}

impl DiffError {
    fn new(line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "patch error at line {line}: {}", self.message),
            None => write!(f, "patch error: {}", self.message),
        }
    }
}

impl std::error::Error for DiffError {}

/// Compute a line-by-line diff between `old` and `new` with default [`DiffOptions`].
pub fn diff_lines<'a>(old: &'a str, new: &'a str) -> Vec<DiffOp<&'a str>> {
    diff_lines_with(old, new, DiffOptions::new())
}

/// Compute a line-by-line diff between `old` and `new` with custom [`DiffOptions`].
pub fn diff_lines_with<'a>(
    old: &'a str,
    new: &'a str,
    options: DiffOptions,
) -> Vec<DiffOp<&'a str>> {
    let (old_lines, _) = split_lines_preserve(old);
    let (new_lines, _) = split_lines_preserve(new);

    let old_hashed: Vec<HashedLine<'a>> = old_lines.into_iter().map(HashedLine::new).collect();
    let new_hashed: Vec<HashedLine<'a>> = new_lines.into_iter().map(HashedLine::new).collect();

    diff_slices_mapped(&old_hashed, &new_hashed, options, |h| h.line)
}

/// Compute a Unicode scalar (`char`) diff between `old` and `new`.
pub fn diff_chars(old: &str, new: &str) -> Vec<DiffOp<char>> {
    diff_chars_with(old, new, DiffOptions::new())
}

/// Compute a Unicode scalar (`char`) diff between `old` and `new` with custom [`DiffOptions`].
pub fn diff_chars_with(old: &str, new: &str, options: DiffOptions) -> Vec<DiffOp<char>> {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    diff_slices_mapped(&old_chars, &new_chars, options, |&c| c)
}

/// Compute a UAX #29 Extended Grapheme Cluster diff between `old` and `new`.
pub fn diff_graphemes<'a>(old: &'a str, new: &'a str) -> Vec<DiffOp<&'a str>> {
    diff_graphemes_with(old, new, DiffOptions::new())
}

/// Compute a UAX #29 Extended Grapheme Cluster diff with custom [`DiffOptions`].
pub fn diff_graphemes_with<'a>(
    old: &'a str,
    new: &'a str,
    options: DiffOptions,
) -> Vec<DiffOp<&'a str>> {
    let old_g: Vec<&'a str> = graphemes(old).collect();
    let new_g: Vec<&'a str> = graphemes(new).collect();
    diff_slices_mapped(&old_g, &new_g, options, |&s| s)
}

/// Compute a diff over arbitrary slices of `T: Eq`.
pub fn diff_slices<'a, T: Eq>(
    old: &'a [T],
    new: &'a [T],
    options: DiffOptions,
) -> Vec<DiffOp<&'a T>> {
    let mut raw = Vec::with_capacity(old.len().max(new.len()));
    let mut state = MyersState::new(options);
    myers_top(old, new, &mut state, &mut raw);
    raw
}

fn diff_slices_mapped<'a, T: Eq, U>(
    old: &'a [T],
    new: &'a [T],
    options: DiffOptions,
    map: impl Fn(&'a T) -> U,
) -> Vec<DiffOp<U>> {
    let raw = diff_slices(old, new, options);
    raw.into_iter()
        .map(|op| match op {
            DiffOp::Equal(x) => DiffOp::Equal(map(x)),
            DiffOp::Insert(x) => DiffOp::Insert(map(x)),
            DiffOp::Delete(x) => DiffOp::Delete(map(x)),
        })
        .collect()
}

/// Group line diff operations into unified diff [`Hunk`]s with `context_lines` surrounding context.
pub fn hunks<'a>(ops: &[DiffOp<&'a str>], context_lines: usize) -> Vec<Hunk<&'a str>> {
    if ops.is_empty() || ops.iter().all(|op| matches!(op, DiffOp::Equal(_))) {
        return Vec::new();
    }

    // Track 1-based line positions before each op.
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut annotated: Vec<(usize, usize, DiffOp<&'a str>)> = Vec::with_capacity(ops.len());
    for &op in ops {
        annotated.push((old_line, new_line, op));
        match op {
            DiffOp::Equal(_) => {
                old_line += 1;
                new_line += 1;
            }
            DiffOp::Delete(_) => {
                old_line += 1;
            }
            DiffOp::Insert(_) => {
                new_line += 1;
            }
        }
    }

    // Find change ranges in `annotated` and merge ranges within `2 * context_lines`.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < annotated.len() {
        if matches!(annotated[i].2, DiffOp::Equal(_)) {
            i += 1;
            continue;
        }
        let change_start = i;
        while i < annotated.len() && !matches!(annotated[i].2, DiffOp::Equal(_)) {
            i += 1;
        }
        let change_end = i;
        let ctx_start = change_start.saturating_sub(context_lines);
        let ctx_end = (change_end + context_lines).min(annotated.len());

        if let Some(last) = ranges.last_mut() {
            if ctx_start <= last.1 {
                last.1 = ctx_end;
            } else {
                ranges.push((ctx_start, ctx_end));
            }
        } else {
            ranges.push((ctx_start, ctx_end));
        }
    }

    let mut result = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        let slice = &annotated[start..end];
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        let mut hunk_ops = Vec::with_capacity(slice.len());

        for &(_, _, op) in slice {
            match op {
                DiffOp::Equal(_) => {
                    old_count += 1;
                    new_count += 1;
                }
                DiffOp::Delete(_) => old_count += 1,
                DiffOp::Insert(_) => new_count += 1,
            }
            hunk_ops.push(op);
        }

        let first_old = slice[0].0;
        let first_new = slice[0].1;
        let old_start = if old_count == 0 {
            first_old.saturating_sub(1)
        } else {
            first_old
        };
        let new_start = if new_count == 0 {
            first_new.saturating_sub(1)
        } else {
            first_new
        };

        result.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            ops: hunk_ops,
        });
    }

    result
}

/// Produce a standard unified diff string between `old` and `new`.
///
/// Returns an empty string when `old == new`.
pub fn unified_diff(
    old: &str,
    new: &str,
    old_label: &str,
    new_label: &str,
    context_lines: usize,
) -> String {
    unified_diff_with(
        old,
        new,
        old_label,
        new_label,
        context_lines,
        DiffOptions::new(),
    )
}

/// Produce a standard unified diff string with custom [`DiffOptions`].
///
/// Line terminators are preserved byte-for-byte: `\r\n`, `\n`, and lone `\r` inside a line
/// are emitted verbatim, and a final line without `\n` is followed by the standard
/// `\ No newline at end of file` marker.
pub fn unified_diff_with(
    old: &str,
    new: &str,
    old_label: &str,
    new_label: &str,
    context_lines: usize,
    options: DiffOptions,
) -> String {
    if old == new {
        return String::new();
    }

    let old_lines = split_lines_inclusive(old);
    let new_lines = split_lines_inclusive(new);

    let old_hashed: Vec<HashedLine<'_>> = old_lines.iter().copied().map(HashedLine::new).collect();
    let new_hashed: Vec<HashedLine<'_>> = new_lines.iter().copied().map(HashedLine::new).collect();
    let ops = diff_slices_mapped(&old_hashed, &new_hashed, options, |h| h.line);
    let hunk_list = hunks(&ops, context_lines);

    if hunk_list.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("--- ");
    out.push_str(old_label);
    out.push('\n');
    out.push_str("+++ ");
    out.push_str(new_label);
    out.push('\n');

    // Buffer Insert ops following Delete ops so each block emits '-' before '+'.
    let mut pending_deletes: Vec<&str> = Vec::new();
    let mut pending_inserts: Vec<&str> = Vec::new();
    let flush = |out: &mut String, deletes: &mut Vec<&str>, inserts: &mut Vec<&str>| {
        for line in deletes.drain(..) {
            push_patch_line(out, '-', line);
        }
        for line in inserts.drain(..) {
            push_patch_line(out, '+', line);
        }
    };

    for hunk in hunk_list {
        out.push_str("@@ -");
        push_range(&mut out, hunk.old_start, hunk.old_count);
        out.push_str(" +");
        push_range(&mut out, hunk.new_start, hunk.new_count);
        out.push_str(" @@\n");

        for op in hunk.ops {
            match op {
                DiffOp::Equal(line) => {
                    flush(&mut out, &mut pending_deletes, &mut pending_inserts);
                    push_patch_line(&mut out, ' ', line);
                }
                DiffOp::Delete(line) => pending_deletes.push(line),
                DiffOp::Insert(line) => pending_inserts.push(line),
            }
        }
        flush(&mut out, &mut pending_deletes, &mut pending_inserts);
    }

    out
}

const NO_EOL_MARKER: &str = "\\ No newline at end of file";

/// Emit one hunk body line. `line` includes its terminator (if any).
fn push_patch_line(out: &mut String, tag: char, line: &str) {
    out.push(tag);
    out.push_str(line);
    if !line.ends_with('\n') {
        out.push('\n');
        out.push_str(NO_EOL_MARKER);
        out.push('\n');
    }
}

/// Strip one trailing `\n` (and a preceding `\r`) for parsing structural patch lines.
fn strip_eol(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn is_no_eol_marker(line: &str) -> bool {
    strip_eol(line) == NO_EOL_MARKER
}

/// Apply a unified diff `patch` to `base`, returning the patched string.
///
/// Line terminators are matched and reproduced exactly (`\n`, `\r\n`, lone `\r`, and
/// missing final newlines via the `\ No newline at end of file` marker).
pub fn apply_patch(base: &str, patch: &str) -> Result<String, DiffError> {
    if patch.trim().is_empty() {
        return Ok(base.to_string());
    }

    let base_lines = split_lines_inclusive(base);
    let patch_lines = split_lines_inclusive(patch);

    let mut idx = 0usize;
    while idx < patch_lines.len() && !strip_eol(patch_lines[idx]).starts_with("@@ ") {
        if strip_eol(patch_lines[idx]).starts_with("--- ") {
            idx += 1;
            if idx < patch_lines.len() && strip_eol(patch_lines[idx]).starts_with("+++ ") {
                idx += 1;
            }
            break;
        }
        idx += 1;
    }

    if idx >= patch_lines.len() || !strip_eol(patch_lines[idx]).starts_with("@@ ") {
        return Err(DiffError::new(Some(1), "missing '@@' hunk header"));
    }

    let mut out = String::with_capacity(base.len());
    let mut cursor = 0usize; // 0-based index into base_lines

    while idx < patch_lines.len() {
        let header = strip_eol(patch_lines[idx]);
        if !header.starts_with("@@ ") {
            idx += 1;
            continue;
        }
        let header_line_no = idx + 1;
        let (old_start, old_count, _new_start, new_count) =
            parse_hunk_header(header).ok_or_else(|| {
                DiffError::new(
                    Some(header_line_no),
                    format!("invalid hunk header: {header}"),
                )
            })?;
        idx += 1;

        let target_start = if old_count == 0 {
            old_start
        } else {
            old_start.checked_sub(1).ok_or_else(|| {
                DiffError::new(
                    Some(header_line_no),
                    "hunk old_start cannot be 0 when old_count > 0",
                )
            })?
        };

        if target_start < cursor || target_start > base_lines.len() {
            return Err(DiffError::new(
                Some(header_line_no),
                "hunk start out of bounds or overlapping previous hunk",
            ));
        }

        while cursor < target_start {
            out.push_str(base_lines[cursor]);
            cursor += 1;
        }

        let mut seen_old = 0usize;
        let mut seen_new = 0usize;

        while idx < patch_lines.len() && !strip_eol(patch_lines[idx]).starts_with("@@ ") {
            let raw = patch_lines[idx];
            let line_no = idx + 1;
            if is_no_eol_marker(raw) {
                // Stray marker not directly following a body line: ignore it.
                idx += 1;
                continue;
            }
            if strip_eol(raw).is_empty() {
                return Err(DiffError::new(
                    Some(line_no),
                    "unexpected empty line in hunk",
                ));
            }
            // `raw` is non-empty, so it has a first char.
            let tag = raw.chars().next().unwrap_or(' ');
            let body = &raw[tag.len_utf8()..];
            idx += 1;

            // The body keeps its exact terminator. A following marker means the line has no
            // trailing `\n`; a patch whose very last line lacks `\n` is treated as a full line.
            let owned: String;
            let content: &str = if patch_lines.get(idx).is_some_and(|l| is_no_eol_marker(l)) {
                idx += 1;
                body.strip_suffix('\n').unwrap_or(body)
            } else if body.ends_with('\n') {
                body
            } else {
                owned = format!("{body}\n");
                &owned
            };

            match tag {
                ' ' => {
                    if base_lines.get(cursor) != Some(&content) {
                        return Err(DiffError::new(Some(line_no), "context line mismatch"));
                    }
                    out.push_str(content);
                    cursor += 1;
                    seen_old += 1;
                    seen_new += 1;
                }
                '-' => {
                    if base_lines.get(cursor) != Some(&content) {
                        return Err(DiffError::new(Some(line_no), "deleted line mismatch"));
                    }
                    cursor += 1;
                    seen_old += 1;
                }
                '+' => {
                    out.push_str(content);
                    seen_new += 1;
                }
                other => {
                    return Err(DiffError::new(
                        Some(line_no),
                        format!("unexpected hunk prefix '{other}'"),
                    ));
                }
            }
            if seen_old == old_count && seen_new == new_count {
                break;
            }
        }

        if seen_old != old_count || seen_new != new_count {
            return Err(DiffError::new(
                Some(header_line_no),
                format!(
                    "hunk line count mismatch: expected -{old_count}/+{new_count}, found -{seen_old}/+{seen_new}"
                ),
            ));
        }
    }

    for line in &base_lines[cursor..] {
        out.push_str(line);
    }
    Ok(out)
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize, usize, usize)> {
    let body = header.strip_prefix("@@ ")?.split(" @@").next()?;
    let (old_part, new_part) = body.split_once(' ')?;
    let old_str = old_part.strip_prefix('-')?;
    let new_str = new_part.strip_prefix('+')?;
    let (os, oc) = parse_range(old_str)?;
    let (ns, nc) = parse_range(new_str)?;
    Some((os, oc, ns, nc))
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    if let Some((start, count)) = s.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

fn push_range(out: &mut String, start: usize, count: usize) {
    use std::fmt::Write as _;
    if count == 1 {
        let _ = write!(out, "{start}");
    } else {
        let _ = write!(out, "{start},{count}");
    }
}

/// Split `s` into lines that keep their exact `\n` terminator (a `\r` before it stays part of
/// the line). The final line has no terminator when `s` does not end with `\n`.
fn split_lines_inclusive(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

fn split_lines_preserve(s: &str) -> (Vec<&str>, bool) {
    if s.is_empty() {
        return (Vec::new(), true);
    }
    let has_trailing_nl = s.ends_with('\n');
    let trimmed = if has_trailing_nl {
        &s[..s.len() - 1]
    } else {
        s
    };
    let lines: Vec<&str> = trimmed.split('\n').collect();
    (lines, has_trailing_nl)
}

#[derive(Clone, Copy)]
struct HashedLine<'a> {
    hash: u64,
    line: &'a str,
}

impl<'a> HashedLine<'a> {
    fn new(line: &'a str) -> Self {
        let mut hasher = FnvHasher::default();
        line.hash(&mut hasher);
        Self {
            hash: hasher.finish(),
            line,
        }
    }
}

impl PartialEq for HashedLine<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.line == other.line
    }
}

impl Eq for HashedLine<'_> {}

#[derive(Default)]
struct FnvHasher(u64);

impl Hasher for FnvHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf29ce484222325
        } else {
            self.0
        };
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        self.0 = h;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

struct MyersState {
    options: DiffOptions,
    steps_used: usize,
    vf: Vec<isize>,
    vb: Vec<isize>,
}

impl MyersState {
    fn new(options: DiffOptions) -> Self {
        Self {
            options,
            steps_used: 0,
            vf: Vec::new(),
            vb: Vec::new(),
        }
    }
}

fn myers_top<'a, T: Eq>(
    old: &'a [T],
    new: &'a [T],
    state: &mut MyersState,
    out: &mut Vec<DiffOp<&'a T>>,
) {
    let prefix = common_prefix_len(old, new);
    for item in &old[..prefix] {
        out.push(DiffOp::Equal(item));
    }
    let old_rem = &old[prefix..];
    let new_rem = &new[prefix..];
    let suffix = common_suffix_len(old_rem, new_rem);

    let old_mid = &old_rem[..old_rem.len() - suffix];
    let new_mid = &new_rem[..new_rem.len() - suffix];

    myers_divide(old_mid, new_mid, state, out);

    for item in &old_rem[old_rem.len() - suffix..] {
        out.push(DiffOp::Equal(item));
    }
}

fn myers_divide<'a, T: Eq>(
    a: &'a [T],
    b: &'a [T],
    state: &mut MyersState,
    out: &mut Vec<DiffOp<&'a T>>,
) {
    let prefix = common_prefix_len(a, b);
    for item in &a[..prefix] {
        out.push(DiffOp::Equal(item));
    }
    let a = &a[prefix..];
    let b = &b[prefix..];
    let suffix = common_suffix_len(a, b);
    let a_mid = &a[..a.len() - suffix];
    let b_mid = &b[..b.len() - suffix];
    let a_suf = &a[a.len() - suffix..];

    if a_mid.is_empty() {
        for item in b_mid {
            out.push(DiffOp::Insert(item));
        }
    } else if b_mid.is_empty() {
        for item in a_mid {
            out.push(DiffOp::Delete(item));
        }
    } else if let Some(snake) = find_middle_snake(a_mid, b_mid, state) {
        myers_divide(&a_mid[..snake.x], &b_mid[..snake.y], state, out);
        for item in &a_mid[snake.x..snake.u] {
            out.push(DiffOp::Equal(item));
        }
        myers_divide(&a_mid[snake.u..], &b_mid[snake.v..], state, out);
    } else {
        // Bounded fallback: emit Delete then Insert on the unmatched middle slice.
        for item in a_mid {
            out.push(DiffOp::Delete(item));
        }
        for item in b_mid {
            out.push(DiffOp::Insert(item));
        }
    }

    for item in a_suf {
        out.push(DiffOp::Equal(item));
    }
}

struct MiddleSnake {
    x: usize,
    y: usize,
    u: usize,
    v: usize,
}

fn find_middle_snake<T: Eq>(a: &[T], b: &[T], state: &mut MyersState) -> Option<MiddleSnake> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let delta = n - m;
    let odd = (delta & 1) != 0;

    let max_d = ((n + m + 1) / 2) as usize;
    let mut d_limit = match state.options.max_edit_distance {
        Some(budget) => max_d.min(budget / 2 + 1),
        None => max_d,
    };
    if let Some(max_steps) = state.options.max_steps {
        // Every round `d` costs at least one step, so no more than `remaining` rounds can run.
        let remaining = max_steps.saturating_sub(state.steps_used);
        if remaining == 0 {
            return None;
        }
        d_limit = d_limit.min(remaining);
    }

    // Size the diagonal vectors by the enforced budget (not by `n + m`), *before* allocating:
    // only diagonals `-(d_limit + 1)..=(d_limit + 1)` are ever touched.
    let vec_len = 2 * d_limit + 3;
    if state.vf.len() < vec_len {
        state.vf.resize(vec_len, 0);
        state.vb.resize(vec_len, 0);
    }
    let offset = (d_limit + 1) as isize;
    state.vf[(offset + 1) as usize] = 0;
    state.vb[(offset - 1) as usize] = n;

    for d in 0..=d_limit {
        let d_i = d as isize;

        // Forward pass
        let mut k = -d_i;
        while k <= d_i {
            if let Some(max_steps) = state.options.max_steps {
                if state.steps_used >= max_steps {
                    return None;
                }
                state.steps_used += 1;
            }

            let mut x = if k == -d_i
                || (k != d_i
                    && state.vf[(offset + k - 1) as usize] < state.vf[(offset + k + 1) as usize])
            {
                state.vf[(offset + k + 1) as usize]
            } else {
                state.vf[(offset + k - 1) as usize] + 1
            };
            let mut y = x - k;
            let x_start = x;
            let y_start = y;

            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            state.vf[(offset + k) as usize] = x;

            if odd {
                let c = k - delta;
                if c >= -(d_i - 1) && c <= (d_i - 1) && x >= state.vb[(offset + c) as usize] {
                    let est_d = 2 * d - 1;
                    if state
                        .options
                        .max_edit_distance
                        .is_some_and(|budget| est_d > budget)
                    {
                        return None;
                    }
                    return Some(MiddleSnake {
                        x: x_start as usize,
                        y: y_start as usize,
                        u: x as usize,
                        v: y as usize,
                    });
                }
            }
            k += 2;
        }

        // Backward pass (iterating c from d down to -d prefers Delete before Insert on ties)
        let mut c = d_i;
        while c >= -d_i {
            if let Some(max_steps) = state.options.max_steps {
                if state.steps_used >= max_steps {
                    return None;
                }
                state.steps_used += 1;
            }

            let mut x = if c == d_i
                || (c != -d_i
                    && state.vb[(offset + c - 1) as usize] < state.vb[(offset + c + 1) as usize])
            {
                state.vb[(offset + c - 1) as usize]
            } else {
                state.vb[(offset + c + 1) as usize] - 1
            };
            let mut y = x - (c + delta);
            let u = x;
            let v = y;

            while x > 0 && y > 0 && a[(x - 1) as usize] == b[(y - 1) as usize] {
                x -= 1;
                y -= 1;
            }
            state.vb[(offset + c) as usize] = x;

            if !odd {
                let k = c + delta;
                if k >= -d_i && k <= d_i && state.vf[(offset + k) as usize] >= x {
                    let est_d = 2 * d;
                    if state
                        .options
                        .max_edit_distance
                        .is_some_and(|budget| est_d > budget)
                    {
                        return None;
                    }
                    return Some(MiddleSnake {
                        x: x as usize,
                        y: y as usize,
                        u: u as usize,
                        v: v as usize,
                    });
                }
            }
            c -= 2;
        }
    }

    None
}

#[inline]
fn common_prefix_len<T: Eq>(a: &[T], b: &[T]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

#[inline]
fn common_suffix_len<T: Eq>(a: &[T], b: &[T]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}
