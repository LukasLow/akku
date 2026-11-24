//! Zero-dependency command-line argument parser with subcommands, global flags,
//! environment fallbacks, and automatic help/version rendering.
//!
//! # Examples
//!
//! ```
//! use akku::cli::{Arg, Command, ValueSource};
//!
//! let cmd = Command::new("serve")
//!     .version("1.0.0")
//!     .about("Run a web server")
//!     .arg(Arg::count("verbose").short('v').long("verbose").global(true))
//!     .arg(
//!         Arg::option("port")
//!             .short('p')
//!             .long("port")
//!             .default_value("3000")
//!             .env("PORT"),
//!     );
//!
//! let matches = cmd.parse_from(["serve", "-vv", "-p8080"]).unwrap();
//! assert_eq!(matches.get_count("verbose"), 2);
//! assert_eq!(matches.get_parsed::<u16>("port").unwrap(), Some(8080));
//! assert_eq!(matches.value_source("port"), Some(ValueSource::Cli));
//! ```

use std::collections::{HashMap, HashSet};
use std::error::Error as StdError;
use std::ffi::OsString;
use std::fmt;
use std::str::FromStr;

/// Alias for [`Command`] representing a top-level CLI application.
pub type App = Command;

/// Where a matched argument value originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueSource {
    /// Supplied directly on the command line.
    Cli,
    /// Loaded from an environment variable.
    Env,
    /// Filled from the argument's configured default value.
    Default,
}

/// Categories of CLI parsing or specification errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Help text was requested via `-h` or `--help`.
    DisplayHelp,
    /// Version output was requested via `-V` or `--version`.
    DisplayVersion,
    /// An unrecognized flag or option was supplied.
    UnknownArgument,
    /// An unrecognized subcommand name was supplied.
    UnknownSubcommand,
    /// An option expected a value but none was provided.
    MissingValue,
    /// A required argument or subcommand was not provided.
    MissingRequired,
    /// An extra positional argument or unexpected value was provided.
    UnexpectedArgument,
    /// A value failed type parsing (for example in [`Matches::get_parsed`]).
    InvalidValue,
    /// An OS argument was not valid UTF-8.
    InvalidUtf8,
    /// The command specification itself is invalid (for example, duplicate flags).
    InvalidSpec,
}

/// Error returned by CLI parsing or typed value conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the error category.
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the conventional process exit code (`0` for help/version, `2` for errors).
    pub const fn exit_code(&self) -> i32 {
        if self.is_help_or_version() {
            0
        } else {
            2
        }
    }

    /// Returns `true` if this error represents a `--help` or `--version` display request.
    pub const fn is_help_or_version(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
        )
    }

    /// Prints this error (to stdout for help/version, stderr otherwise) and exits the process.
    pub fn exit(&self) -> ! {
        if self.is_help_or_version() {
            print!("{}", self.message);
        } else {
            eprintln!("error: {}", self.message);
        }
        std::process::exit(self.exit_code());
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    Flag,
    Count,
    Option,
    Positional,
}

/// Specification of a single command-line argument (flag, counter, option, or positional).
#[derive(Debug, Clone)]
pub struct Arg {
    name: String,
    kind: ArgKind,
    short: Option<char>,
    long: Option<String>,
    help: Option<String>,
    value_name: Option<String>,
    default_value: Option<String>,
    env: Option<String>,
    required: bool,
    multiple: bool,
    variadic: bool,
    global: bool,
    allow_hyphen_values: bool,
}

impl Arg {
    fn new_kind(name: impl Into<String>, kind: ArgKind) -> Self {
        Self {
            name: name.into(),
            kind,
            short: None,
            long: None,
            help: None,
            value_name: None,
            default_value: None,
            env: None,
            required: false,
            multiple: false,
            variadic: false,
            global: false,
            allow_hyphen_values: false,
        }
    }

    /// Creates a boolean flag argument (present = `true`, absent = `false`).
    pub fn flag(name: impl Into<String>) -> Self {
        Self::new_kind(name, ArgKind::Flag)
    }

    /// Creates a counting flag argument (counts occurrences, such as `-vvv` -> `3`).
    pub fn count(name: impl Into<String>) -> Self {
        Self::new_kind(name, ArgKind::Count)
    }

    /// Creates a key-value option argument (for example, `--port 8080` or `-p8080`).
    pub fn option(name: impl Into<String>) -> Self {
        Self::new_kind(name, ArgKind::Option)
    }

    /// Creates a positional argument.
    pub fn positional(name: impl Into<String>) -> Self {
        Self::new_kind(name, ArgKind::Positional)
    }

    /// Sets the single-character short flag (for example, `'p'` for `-p`).
    pub fn short(mut self, c: char) -> Self {
        self.short = Some(c);
        self
    }

    /// Sets the long flag name without leading `--` (for example, `"port"` for `--port`).
    pub fn long(mut self, name: impl Into<String>) -> Self {
        self.long = Some(name.into());
        self
    }

    /// Sets the help description shown in rendered usage text.
    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.help = Some(text.into());
        self
    }

    /// Sets the placeholder name shown for values in help text (for example, `FILE`).
    pub fn value_name(mut self, name: impl Into<String>) -> Self {
        self.value_name = Some(name.into());
        self
    }

    /// Sets a default value used when neither CLI nor environment provides one.
    pub fn default_value(mut self, val: impl Into<String>) -> Self {
        self.default_value = Some(val.into());
        self
    }

    /// Sets an environment variable name consulted as a fallback when absent from CLI.
    pub fn env(mut self, var: impl Into<String>) -> Self {
        self.env = Some(var.into());
        self
    }

    /// Marks whether this argument must be provided.
    pub fn required(mut self, yes: bool) -> Self {
        self.required = yes;
        self
    }

    /// Allows an option or positional argument to collect multiple values.
    pub fn multiple(mut self, yes: bool) -> Self {
        self.multiple = yes;
        self
    }

    /// Marks a positional or option argument as accepting multiple values (alias for multi-value capture).
    pub fn variadic(mut self, yes: bool) -> Self {
        self.variadic = yes;
        self
    }

    /// Propagates this flag or option across all nested subcommands.
    pub fn global(mut self, yes: bool) -> Self {
        self.global = yes;
        self
    }

    /// Allows option values beginning with `-` (such as negative numbers like `-5`).
    pub fn allow_hyphen_values(mut self, yes: bool) -> Self {
        self.allow_hyphen_values = yes;
        self
    }

    fn effective_long(&self) -> Option<&str> {
        if self.kind == ArgKind::Positional {
            None
        } else if let Some(ref l) = self.long {
            Some(l.as_str())
        } else if self.short.is_none() {
            Some(self.name.as_str())
        } else {
            None
        }
    }

    fn accepts_many(&self) -> bool {
        self.multiple || self.variadic
    }
}

#[derive(Debug, Clone)]
struct MatchedArg {
    values: Vec<String>,
    count: u64,
    flag: bool,
    source: ValueSource,
}

/// Parsed argument values and matched subcommand tree.
#[derive(Debug, Clone, Default)]
pub struct Matches {
    args: HashMap<String, MatchedArg>,
    subcommand: Option<(String, Box<Matches>)>,
}

impl Matches {
    /// Returns `true` if the named flag was enabled.
    pub fn get_flag(&self, name: &str) -> bool {
        self.args.get(name).is_some_and(|m| m.flag)
    }

    /// Returns the number of occurrences for a counting (or boolean) flag.
    pub fn get_count(&self, name: &str) -> u64 {
        self.args.get(name).map_or(0, |m| m.count)
    }

    /// Returns the last (or single) string value matched for `name`.
    pub fn get_str(&self, name: &str) -> Option<&str> {
        self.args
            .get(name)
            .and_then(|m| m.values.last().map(String::as_str))
    }

    /// Returns all matched string values for `name`, if any were provided.
    pub fn get_many(&self, name: &str) -> Option<Vec<&str>> {
        self.args.get(name).and_then(|m| {
            if m.values.is_empty() {
                None
            } else {
                Some(m.values.iter().map(String::as_str).collect())
            }
        })
    }

    /// Parses the matched string value for `name` into `T`.
    pub fn get_parsed<T>(&self, name: &str) -> Result<Option<T>, Error>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        match self.get_str(name) {
            None => Ok(None),
            Some(raw) => raw.parse::<T>().map(Some).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidValue,
                    format!("invalid value {raw:?} for '{name}': {e}"),
                )
            }),
        }
    }

    /// Parses all matched string values for `name` into `Vec<T>`.
    pub fn get_many_parsed<T>(&self, name: &str) -> Result<Option<Vec<T>>, Error>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        let Some(vals) = self.get_many(name) else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(vals.len());
        for raw in vals {
            let parsed = raw.parse::<T>().map_err(|e| {
                Error::new(
                    ErrorKind::InvalidValue,
                    format!("invalid value {raw:?} for '{name}': {e}"),
                )
            })?;
            out.push(parsed);
        }
        Ok(Some(out))
    }

    /// Returns where the matched value for `name` originated.
    pub fn value_source(&self, name: &str) -> Option<ValueSource> {
        self.args.get(name).map(|m| m.source)
    }

    /// Returns the selected subcommand name and its parsed [`Matches`], if any.
    pub fn subcommand(&self) -> Option<(&str, &Matches)> {
        self.subcommand
            .as_ref()
            .map(|(name, m)| (name.as_str(), m.as_ref()))
    }
}

/// A CLI command or subcommand specification.
#[derive(Debug, Clone)]
pub struct Command {
    name: String,
    version: Option<String>,
    about: Option<String>,
    args: Vec<Arg>,
    subcommands: Vec<Command>,
    subcommand_required: bool,
    disable_help_flag: bool,
    disable_version_flag: bool,
}

impl Command {
    /// Creates a new `Command` with the given program or subcommand name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
            about: None,
            args: Vec::new(),
            subcommands: Vec::new(),
            subcommand_required: false,
            disable_help_flag: false,
            disable_version_flag: false,
        }
    }

    /// Sets the command's version string, enabling automatic `-V` / `--version`.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Sets a brief description displayed in rendered help text.
    pub fn about(mut self, about: impl Into<String>) -> Self {
        self.about = Some(about.into());
        self
    }

    /// Registers an [`Arg`] on this command.
    pub fn arg(mut self, arg: Arg) -> Self {
        self.args.push(arg);
        self
    }

    /// Registers a nested subcommand.
    pub fn subcommand(mut self, cmd: Command) -> Self {
        self.subcommands.push(cmd);
        self
    }

    /// Requires that a subcommand be invoked.
    pub fn subcommand_required(mut self, yes: bool) -> Self {
        self.subcommand_required = yes;
        self
    }

    /// Disables automatic `-h` and `--help` handling.
    pub fn disable_help_flag(mut self, yes: bool) -> Self {
        self.disable_help_flag = yes;
        self
    }

    /// Disables automatic `-V` and `--version` handling.
    pub fn disable_version_flag(mut self, yes: bool) -> Self {
        self.disable_version_flag = yes;
        self
    }

    /// Parses process arguments from [`std::env::args_os`], exiting on error or help/version.
    pub fn parse(&self) -> Matches {
        match self.try_parse() {
            Ok(m) => m,
            Err(e) => e.exit(),
        }
    }

    /// Parses process arguments from [`std::env::args_os`] and returns a [`Result`].
    pub fn try_parse(&self) -> Result<Matches, Error> {
        self.parse_from(std::env::args_os())
    }

    /// Parses arguments from an iterator (the first element is treated as argv\[0\]),
    /// using process environment variables for `.env(...)` fallbacks.
    pub fn parse_from<I, T>(&self, args: I) -> Result<Matches, Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        self.parse_from_with_env(args, |k| std::env::var(k).ok())
    }

    /// Parses arguments from an iterator using a custom environment lookup closure
    /// for race-free unit testing.
    pub fn parse_from_with_env<I, T, F>(&self, args: I, mut env_lookup: F) -> Result<Matches, Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
        F: FnMut(&str) -> Option<String>,
    {
        let mut tokens = Vec::new();
        for (idx, raw) in args.into_iter().enumerate() {
            let os: OsString = raw.into();
            let s = os
                .into_string()
                .map_err(|_| Error::new(ErrorKind::InvalidUtf8, "argument is not valid UTF-8"))?;
            if idx > 0 {
                tokens.push(s);
            }
        }
        self.parse_tokens(&tokens, &[], &mut env_lookup)
    }

    /// Renders formatted help text for this command.
    pub fn render_help(&self) -> String {
        self.render_help_with_globals(&[])
    }

    /// Renders formatted version text for this command.
    pub fn render_version(&self) -> String {
        match &self.version {
            Some(v) => format!("{} {v}\n", self.name),
            None => format!("{}\n", self.name),
        }
    }

    fn validate_spec(&self, inherited_globals: &[Arg]) -> Result<Vec<Arg>, Error> {
        let mut all_args = Vec::with_capacity(self.args.len() + inherited_globals.len());
        let mut seen_names = HashSet::new();
        let mut seen_shorts = HashSet::new();
        let mut seen_longs = HashSet::new();

        for arg in &self.args {
            if arg.kind == ArgKind::Positional && (arg.short.is_some() || arg.long.is_some()) {
                return Err(Error::new(
                    ErrorKind::InvalidSpec,
                    format!(
                        "positional argument '{}' cannot have short or long flags",
                        arg.name
                    ),
                ));
            }
            if !seen_names.insert(arg.name.as_str()) {
                return Err(Error::new(
                    ErrorKind::InvalidSpec,
                    format!("duplicate argument name '{}'", arg.name),
                ));
            }
            if let Some(c) = arg.short {
                if !seen_shorts.insert(c) {
                    return Err(Error::new(
                        ErrorKind::InvalidSpec,
                        format!("duplicate short flag '-{c}'"),
                    ));
                }
            }
            if let Some(l) = arg.effective_long() {
                if !seen_longs.insert(l) {
                    return Err(Error::new(
                        ErrorKind::InvalidSpec,
                        format!("duplicate long flag '--{l}'"),
                    ));
                }
            }
            all_args.push(arg.clone());
        }

        // Merge inherited global args that are not shadowed locally.
        for g in inherited_globals {
            if seen_names.insert(g.name.as_str()) {
                if let Some(c) = g.short {
                    if !seen_shorts.insert(c) {
                        return Err(Error::new(
                            ErrorKind::InvalidSpec,
                            format!("global short flag '-{c}' conflicts with local flag"),
                        ));
                    }
                }
                if let Some(l) = g.effective_long() {
                    if !seen_longs.insert(l) {
                        return Err(Error::new(
                            ErrorKind::InvalidSpec,
                            format!("global long flag '--{l}' conflicts with local flag"),
                        ));
                    }
                }
                all_args.push(g.clone());
            }
        }

        let mut seen_subs = HashSet::new();
        for sub in &self.subcommands {
            if !seen_subs.insert(sub.name.as_str()) {
                return Err(Error::new(
                    ErrorKind::InvalidSpec,
                    format!("duplicate subcommand '{}'", sub.name),
                ));
            }
        }

        Ok(all_args)
    }

    fn render_help_with_globals(&self, inherited_globals: &[Arg]) -> String {
        let mut out = String::new();
        if let Some(ref about) = self.about {
            out.push_str(about);
            out.push_str("\n\n");
        }

        // Usage line.
        out.push_str("Usage: ");
        out.push_str(&self.name);

        let all_args: Vec<&Arg> = self
            .args
            .iter()
            .chain(
                inherited_globals
                    .iter()
                    .filter(|g| !self.args.iter().any(|a| a.name == g.name)),
            )
            .collect();

        let has_options = all_args.iter().any(|a| a.kind != ArgKind::Positional)
            || !self.disable_help_flag
            || (self.version.is_some() && !self.disable_version_flag);
        if has_options {
            out.push_str(" [OPTIONS]");
        }
        for pos in all_args.iter().filter(|a| a.kind == ArgKind::Positional) {
            let label = pos
                .value_name
                .as_deref()
                .unwrap_or(&pos.name)
                .to_uppercase();
            if pos.required {
                out.push_str(&format!(" <{label}>"));
            } else {
                out.push_str(&format!(" [{label}]"));
            }
            if pos.accepts_many() {
                out.push_str("...");
            }
        }
        if !self.subcommands.is_empty() {
            if self.subcommand_required {
                out.push_str(" <COMMAND>");
            } else {
                out.push_str(" [COMMAND]");
            }
        }
        out.push('\n');

        // Positionals section.
        let positionals: Vec<&&Arg> = all_args
            .iter()
            .filter(|a| a.kind == ArgKind::Positional)
            .collect();
        if !positionals.is_empty() {
            out.push_str("\nArguments:\n");
            for pos in positionals {
                let label = pos
                    .value_name
                    .as_deref()
                    .unwrap_or(&pos.name)
                    .to_uppercase();
                out.push_str(&format!("  <{label}>"));
                if let Some(ref h) = pos.help {
                    out.push_str(&format!("  {h}"));
                }
                out.push('\n');
            }
        }

        // Options section.
        let mut opt_lines = Vec::new();
        for arg in all_args.iter().filter(|a| a.kind != ArgKind::Positional) {
            let mut spec = String::new();
            if let Some(c) = arg.short {
                spec.push('-');
                spec.push(c);
                if arg.effective_long().is_some() {
                    spec.push_str(", ");
                }
            }
            if let Some(l) = arg.effective_long() {
                spec.push_str("--");
                spec.push_str(l);
            }
            if arg.kind == ArgKind::Option {
                let vn = arg
                    .value_name
                    .as_deref()
                    .unwrap_or(&arg.name)
                    .to_uppercase();
                spec.push_str(&format!(" <{vn}>"));
            }
            let mut desc = arg.help.clone().unwrap_or_default();
            if let Some(ref def) = arg.default_value {
                if !desc.is_empty() {
                    desc.push(' ');
                }
                desc.push_str(&format!("[default: {def}]"));
            }
            if let Some(ref env) = arg.env {
                if !desc.is_empty() {
                    desc.push(' ');
                }
                desc.push_str(&format!("[env: {env}]"));
            }
            opt_lines.push((spec, desc));
        }
        if !self.disable_help_flag {
            opt_lines.push(("-h, --help".to_string(), "Print help".to_string()));
        }
        if self.version.is_some() && !self.disable_version_flag {
            opt_lines.push(("-V, --version".to_string(), "Print version".to_string()));
        }
        if !opt_lines.is_empty() {
            out.push_str("\nOptions:\n");
            let pad = opt_lines.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
            for (spec, desc) in opt_lines {
                if desc.is_empty() {
                    out.push_str(&format!("  {spec}\n"));
                } else {
                    out.push_str(&format!("  {spec:<pad$}  {desc}\n"));
                }
            }
        }

        // Subcommands section.
        if !self.subcommands.is_empty() {
            out.push_str("\nCommands:\n");
            let pad = self
                .subcommands
                .iter()
                .map(|c| c.name.len())
                .max()
                .unwrap_or(0);
            for sub in &self.subcommands {
                if let Some(ref about) = sub.about {
                    out.push_str(&format!("  {:<pad$}  {about}\n", sub.name));
                } else {
                    out.push_str(&format!("  {}\n", sub.name));
                }
            }
        }

        out
    }

    fn parse_tokens<F>(
        &self,
        tokens: &[String],
        inherited_globals: &[Arg],
        env_lookup: &mut F,
    ) -> Result<Matches, Error>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let all_args = self.validate_spec(inherited_globals)?;
        let positionals: Vec<usize> = all_args
            .iter()
            .enumerate()
            .filter_map(|(idx, a)| (a.kind == ArgKind::Positional).then_some(idx))
            .collect();

        let has_user_short = |c: char| all_args.iter().any(|a| a.short == Some(c));
        let has_user_long = |l: &str| all_args.iter().any(|a| a.effective_long() == Some(l));

        let auto_help_short = !self.disable_help_flag && !has_user_short('h');
        let auto_help_long = !self.disable_help_flag && !has_user_long("help");
        let auto_ver_short =
            self.version.is_some() && !self.disable_version_flag && !has_user_short('V');
        let auto_ver_long =
            self.version.is_some() && !self.disable_version_flag && !has_user_long("version");

        let mut matches = Matches::default();
        let mut pos_cursor = 0usize;
        let mut after_double_dash = false;
        let mut i = 0usize;

        while i < tokens.len() {
            let tok = &tokens[i];

            if !after_double_dash && tok == "--" {
                after_double_dash = true;
                i += 1;
                continue;
            }

            if !after_double_dash && tok.starts_with("--") && tok.len() > 2 {
                let body = &tok[2..];
                let (long_name, inline_val) = match body.split_once('=') {
                    Some((k, v)) => (k, Some(v)),
                    None => (body, None),
                };

                if inline_val.is_none() && auto_help_long && long_name == "help" {
                    return Err(Error::new(
                        ErrorKind::DisplayHelp,
                        self.render_help_with_globals(inherited_globals),
                    ));
                }
                if inline_val.is_none() && auto_ver_long && long_name == "version" {
                    return Err(Error::new(ErrorKind::DisplayVersion, self.render_version()));
                }

                let arg = all_args
                    .iter()
                    .find(|a| a.effective_long() == Some(long_name))
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::UnknownArgument,
                            format!("unknown argument '--{long_name}'"),
                        )
                    })?;

                match arg.kind {
                    ArgKind::Flag => {
                        if inline_val.is_some() {
                            return Err(Error::new(
                                ErrorKind::UnexpectedArgument,
                                format!("flag '--{long_name}' does not take a value"),
                            ));
                        }
                        record_flag(&mut matches, &arg.name, ValueSource::Cli);
                    }
                    ArgKind::Count => {
                        if inline_val.is_some() {
                            return Err(Error::new(
                                ErrorKind::UnexpectedArgument,
                                format!("flag '--{long_name}' does not take a value"),
                            ));
                        }
                        record_count(&mut matches, &arg.name, ValueSource::Cli);
                    }
                    ArgKind::Option => {
                        let val = if let Some(v) = inline_val {
                            v.to_string()
                        } else {
                            i += 1;
                            take_option_value(tokens, i, arg, &format!("--{long_name}"))?
                        };
                        record_value(&mut matches, arg, val, ValueSource::Cli);
                    }
                    ArgKind::Positional => unreachable!(),
                }
                i += 1;
                continue;
            }

            if !after_double_dash && tok.starts_with('-') && tok.len() > 1 {
                let rest = &tok[1..];
                for (byte_idx, c) in rest.char_indices() {
                    if auto_help_short && c == 'h' {
                        return Err(Error::new(
                            ErrorKind::DisplayHelp,
                            self.render_help_with_globals(inherited_globals),
                        ));
                    }
                    if auto_ver_short && c == 'V' {
                        return Err(Error::new(ErrorKind::DisplayVersion, self.render_version()));
                    }

                    let arg = all_args
                        .iter()
                        .find(|a| a.short == Some(c))
                        .ok_or_else(|| {
                            Error::new(
                                ErrorKind::UnknownArgument,
                                format!("unknown argument '-{c}'"),
                            )
                        })?;

                    match arg.kind {
                        ArgKind::Flag => {
                            record_flag(&mut matches, &arg.name, ValueSource::Cli);
                        }
                        ArgKind::Count => {
                            record_count(&mut matches, &arg.name, ValueSource::Cli);
                        }
                        ArgKind::Option => {
                            let remainder = &rest[byte_idx + c.len_utf8()..];
                            let val = if !remainder.is_empty() {
                                remainder.to_string()
                            } else {
                                i += 1;
                                take_option_value(tokens, i, arg, &format!("-{c}"))?
                            };
                            record_value(&mut matches, arg, val, ValueSource::Cli);
                            break;
                        }
                        ArgKind::Positional => unreachable!(),
                    }
                }
                i += 1;
                continue;
            }

            // Non-flag token (or single "-", or after "--"):
            // Check subcommand first if not after `--`.
            if !after_double_dash && !self.subcommands.is_empty() {
                if let Some(sub) = self.subcommands.iter().find(|s| s.name == *tok) {
                    let next_globals: Vec<Arg> =
                        all_args.iter().filter(|a| a.global).cloned().collect();
                    let mut sub_matches =
                        sub.parse_tokens(&tokens[i + 1..], &next_globals, env_lookup)?;

                    // Propagate global arguments bidirectionally (including into nested subcommands):
                    // - CLI globals parsed on parent flow down to subcommand tree unless overridden by subcommand CLI.
                    // - CLI globals parsed on subcommand flow up to parent.
                    for g in &next_globals {
                        let parent_entry = matches.args.get(&g.name).cloned();
                        let child_entry = sub_matches.args.get(&g.name).cloned();
                        match (parent_entry, child_entry) {
                            (Some(p), Some(c)) => {
                                if p.source == ValueSource::Cli && c.source != ValueSource::Cli {
                                    propagate_global_down(&mut sub_matches, &g.name, &p, false);
                                } else if p.source == ValueSource::Cli
                                    && c.source == ValueSource::Cli
                                    && g.kind == ArgKind::Count
                                {
                                    let combined = MatchedArg {
                                        values: Vec::new(),
                                        count: p.count + c.count,
                                        flag: true,
                                        source: ValueSource::Cli,
                                    };
                                    matches.args.insert(g.name.clone(), combined.clone());
                                    propagate_global_down(
                                        &mut sub_matches,
                                        &g.name,
                                        &combined,
                                        true,
                                    );
                                } else if c.source == ValueSource::Cli {
                                    matches.args.insert(g.name.clone(), c);
                                }
                            }
                            (Some(p), None) => {
                                propagate_global_down(&mut sub_matches, &g.name, &p, false);
                            }
                            (None, Some(c)) if c.source == ValueSource::Cli => {
                                matches.args.insert(g.name.clone(), c);
                            }
                            _ => {}
                        }
                    }

                    matches.subcommand = Some((sub.name.clone(), Box::new(sub_matches)));
                    break;
                }
            }

            // Otherwise assign to next positional slot.
            if pos_cursor < positionals.len() {
                let arg = &all_args[positionals[pos_cursor]];
                record_value(&mut matches, arg, tok.clone(), ValueSource::Cli);
                if !arg.accepts_many() {
                    pos_cursor += 1;
                }
                i += 1;
                continue;
            }

            if !self.subcommands.is_empty() && !after_double_dash {
                return Err(Error::new(
                    ErrorKind::UnknownSubcommand,
                    format!("unknown subcommand '{tok}'"),
                ));
            }
            return Err(Error::new(
                ErrorKind::UnexpectedArgument,
                format!("unexpected argument '{tok}'"),
            ));
        }

        // Apply env and default fallbacks and validate required arguments.
        for arg in &all_args {
            if !matches.args.contains_key(&arg.name) {
                if let Some(ref env_var) = arg.env {
                    if let Some(env_val) = env_lookup(env_var) {
                        apply_fallback(&mut matches, arg, env_val, ValueSource::Env);
                    }
                }
            }
            if !matches.args.contains_key(&arg.name) {
                if let Some(ref def_val) = arg.default_value {
                    apply_fallback(&mut matches, arg, def_val.clone(), ValueSource::Default);
                }
            }
            if arg.required && !matches.args.contains_key(&arg.name) {
                return Err(Error::new(
                    ErrorKind::MissingRequired,
                    format!("missing required argument '{}'", arg.name),
                ));
            }
        }

        if self.subcommand_required && matches.subcommand.is_none() {
            return Err(Error::new(
                ErrorKind::MissingRequired,
                format!("a subcommand is required for '{}'", self.name),
            ));
        }

        Ok(matches)
    }
}

fn take_option_value(
    tokens: &[String],
    idx: usize,
    arg: &Arg,
    display_flag: &str,
) -> Result<String, Error> {
    let Some(val) = tokens.get(idx) else {
        return Err(Error::new(
            ErrorKind::MissingValue,
            format!("option '{display_flag}' requires a value"),
        ));
    };
    if !arg.allow_hyphen_values && val.starts_with('-') && val != "-" {
        return Err(Error::new(
            ErrorKind::MissingValue,
            format!("option '{display_flag}' requires a value (got '{val}')"),
        ));
    }
    Ok(val.clone())
}

fn record_flag(matches: &mut Matches, name: &str, source: ValueSource) {
    matches.args.insert(
        name.to_string(),
        MatchedArg {
            values: Vec::new(),
            count: 1,
            flag: true,
            source,
        },
    );
}

fn record_count(matches: &mut Matches, name: &str, source: ValueSource) {
    let entry = matches
        .args
        .entry(name.to_string())
        .or_insert_with(|| MatchedArg {
            values: Vec::new(),
            count: 0,
            flag: false,
            source,
        });
    entry.count += 1;
    entry.flag = true;
    entry.source = source;
}

fn record_value(matches: &mut Matches, arg: &Arg, val: String, source: ValueSource) {
    let entry = matches
        .args
        .entry(arg.name.clone())
        .or_insert_with(|| MatchedArg {
            values: Vec::new(),
            count: 0,
            flag: false,
            source,
        });
    if arg.accepts_many() {
        entry.values.push(val);
    } else {
        entry.values.clear();
        entry.values.push(val);
    }
    entry.count = entry.values.len() as u64;
    entry.source = source;
}

fn apply_fallback(matches: &mut Matches, arg: &Arg, val: String, source: ValueSource) {
    match arg.kind {
        ArgKind::Flag => {
            let truthy = matches!(
                val.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
            matches.args.insert(
                arg.name.clone(),
                MatchedArg {
                    values: Vec::new(),
                    count: u64::from(truthy),
                    flag: truthy,
                    source,
                },
            );
        }
        ArgKind::Count => {
            if let Ok(n) = val.trim().parse::<u64>() {
                matches.args.insert(
                    arg.name.clone(),
                    MatchedArg {
                        values: Vec::new(),
                        count: n,
                        flag: n > 0,
                        source,
                    },
                );
            }
        }
        ArgKind::Option | ArgKind::Positional => {
            record_value(matches, arg, val, source);
        }
    }
}

fn propagate_global_down(m: &mut Matches, name: &str, entry: &MatchedArg, overwrite_cli: bool) {
    let should_write = match m.args.get(name) {
        Some(existing) => overwrite_cli || existing.source != ValueSource::Cli,
        None => true,
    };
    if should_write {
        m.args.insert(name.to_string(), entry.clone());
    }
    if let Some((_, sub)) = &mut m.subcommand {
        propagate_global_down(sub, name, entry, overwrite_cli);
    }
}
