//! [`RegexBuilder`]: flags and compile-time safety limits.

use super::error::Result;
use super::Regex;

/// Default limit for the compiled program (instructions, classes and the
/// per-search capture-slot table), in bytes.
pub(crate) const DEFAULT_SIZE_LIMIT: usize = 10 << 20;
/// Default memory budget of the lazy DFA cache, in bytes.
pub(crate) const DEFAULT_DFA_SIZE_LIMIT: usize = 2 << 20;
/// Default maximum group/repetition nesting depth.
pub(crate) const DEFAULT_NEST_LIMIT: u32 = 250;
/// Hard ceiling for the nesting depth, whatever is configured.
pub(crate) const MAX_NEST_LIMIT: u32 = 1000;
/// Default maximum pattern length, in bytes.
pub(crate) const DEFAULT_PATTERN_LEN_LIMIT: usize = 128 << 10;

/// Resolved compile options.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Config {
    pub(crate) case_insensitive: bool,
    pub(crate) multi_line: bool,
    pub(crate) dot_matches_new_line: bool,
    pub(crate) size_limit: usize,
    pub(crate) nest_limit: u32,
    pub(crate) dfa_size_limit: usize,
    pub(crate) pattern_len_limit: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            case_insensitive: false,
            multi_line: false,
            dot_matches_new_line: false,
            size_limit: DEFAULT_SIZE_LIMIT,
            nest_limit: DEFAULT_NEST_LIMIT,
            dfa_size_limit: DEFAULT_DFA_SIZE_LIMIT,
            pattern_len_limit: DEFAULT_PATTERN_LEN_LIMIT,
        }
    }
}

/// Configures and compiles a [`Regex`].
///
/// Flags set here act as defaults; inline flags in the pattern (`(?i)`,
/// `(?-m)`, ...) override them.
///
/// The three limits protect services that compile untrusted patterns:
///
/// | Limit | Default | Guards against |
/// |---|---|---|
/// | [`pattern_len_limit`](Self::pattern_len_limit) | 128 KiB | huge pattern text |
/// | [`nest_limit`](Self::nest_limit) | 250 (max 1000) | deep nesting / stack exhaustion |
/// | [`size_limit`](Self::size_limit) | 10 MiB | repetition bombs like `(?:a{1000}){1000}` |
///
/// The size check is performed on a saturating *estimate* before any code is
/// emitted, so even absurd patterns fail fast without allocating.
///
/// # Examples
///
/// ```
/// use akku::regex::RegexBuilder;
///
/// let re = RegexBuilder::new(r"^hello$")
///     .case_insensitive(true)
///     .multi_line(true)
///     .build()
///     .unwrap();
/// assert!(re.is_match("say\nHELLO\nworld"));
/// ```
#[derive(Debug, Clone)]
pub struct RegexBuilder {
    pattern: String,
    config: Config,
}

impl RegexBuilder {
    /// Starts a builder for `pattern` with default options.
    pub fn new(pattern: &str) -> RegexBuilder {
        RegexBuilder {
            pattern: pattern.to_owned(),
            config: Config::default(),
        }
    }

    /// Case-insensitive matching by default (`(?i)`).
    pub fn case_insensitive(&mut self, yes: bool) -> &mut Self {
        self.config.case_insensitive = yes;
        self
    }

    /// `^`/`$` match at line boundaries by default (`(?m)`).
    pub fn multi_line(&mut self, yes: bool) -> &mut Self {
        self.config.multi_line = yes;
        self
    }

    /// `.` also matches `\n` by default (`(?s)`).
    pub fn dot_matches_new_line(&mut self, yes: bool) -> &mut Self {
        self.config.dot_matches_new_line = yes;
        self
    }

    /// Maximum compiled program size in bytes (default 10 MiB).
    ///
    /// This bounds the instructions, character classes and the capture slot
    /// table every search may allocate.
    pub fn size_limit(&mut self, bytes: usize) -> &mut Self {
        self.config.size_limit = bytes;
        self
    }

    /// Maximum nesting depth of groups and repetitions (default 250).
    ///
    /// Values above 1000 are clamped to 1000 so that compilation can never
    /// exhaust the stack.
    pub fn nest_limit(&mut self, depth: u32) -> &mut Self {
        self.config.nest_limit = depth.min(MAX_NEST_LIMIT);
        self
    }

    /// Memory budget of the lazy DFA cache in bytes (default 2 MiB).
    ///
    /// When the budget is exhausted the cache is flushed; if that happens
    /// too often the search transparently falls back to the PikeVM, so the
    /// linear-time guarantee is unaffected.
    pub fn dfa_size_limit(&mut self, bytes: usize) -> &mut Self {
        self.config.dfa_size_limit = bytes;
        self
    }

    /// Maximum pattern length in bytes (default 128 KiB).
    pub fn pattern_len_limit(&mut self, bytes: usize) -> &mut Self {
        self.config.pattern_len_limit = bytes;
        self
    }

    /// Compiles the pattern.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`](super::Error) for invalid syntax, unsupported
    /// features or exceeded limits.
    pub fn build(&self) -> Result<Regex> {
        Regex::from_config(&self.pattern, &self.config)
    }
}
