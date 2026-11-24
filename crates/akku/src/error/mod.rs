//! Ergonomic application errors with a context chain and a structured kind.
//!
//! [`Error`] is Akku's take on an `anyhow`-style error type: one error value
//! for application code, built with constructor functions, wrapped layer by
//! layer with context (like Go's `%w`, but without losing history), and fully
//! inspectable as a chain. It is pure safe `std` — no external dependencies,
//! no allocator tricks.
//!
//! # Quick start
//!
//! ```
//! use akku::error::{Context, Error, Result};
//!
//! fn load_config() -> Result<String> {
//!     Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing akku.toml"))
//!         .context("failed to read the configuration file")
//! }
//!
//! let error = load_config().unwrap_err();
//! assert_eq!(error.to_string(), "failed to read the configuration file");
//! assert_eq!(
//!     format!("{:#}", error),
//!     "failed to read the configuration file: missing akku.toml",
//! );
//! assert!(error.is::<std::io::Error>());
//! ```
//!
//! # Ergonomics
//!
//! The [`Context`] trait adds `.context(..)` and `.with_context(..)` to every
//! `Result<T, E>` whose error implements [`std::error::Error`] and to every
//! `Option<T>`, so `?`-chains end in a meaningful [`Error`] instead of a bare
//! leaf. Targeted [`From`] implementations (for `std::io::Error`,
//! `std::fmt::Error`, the UTF-8 errors and the parse errors) keep `?`
//! conversion from the common `std` error types allocation-light and direct.
//!
//! [`bail!`] and [`ensure!`] round out the feel of application-level error
//! handling. They live at [`akku::error`](crate::error) only; the crate root
//! stays namespace-clean (see `Namespace.md`).

/// Convenience alias used across application code.
///
/// The default error type is [`Error`]. Functions that need their own error
/// type can override the second parameter.
///
/// ```
/// use akku::error::Result;
///
/// fn count() -> Result<usize> {
///     Ok(3)
/// }
///
/// assert_eq!(count().unwrap(), 3);
/// ```
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A coarse, structured classification of what went wrong.
///
/// The kind travels with the error: wrapping an error with
/// [`Error::context`] preserves its [`Error::kind`]. This lets callers make
/// decisions (`NotFound` → treat as missing, `InvalidInput` → log and ignore)
/// without parsing message strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The thing does not exist.
    NotFound,
    /// The caller is not allowed to do this.
    PermissionDenied,
    /// A value presented by the caller is malformed or out of range.
    InvalidInput,
    /// The thing already exists and must not be created again.
    AlreadyExists,
    /// An operation exceeded its deadline.
    TimedOut,
    /// A dependency is temporarily not able to serve.
    Unavailable,
    /// Authentication is required or failed.
    Unauthenticated,
    /// Mutually exclusive state was observed.
    Conflict,
    /// A bug or an invariant violation inside the program.
    Internal,
    /// No better classification applies.
    Other,
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ErrorKind::NotFound => "not found",
            ErrorKind::PermissionDenied => "permission denied",
            ErrorKind::InvalidInput => "invalid input",
            ErrorKind::AlreadyExists => "already exists",
            ErrorKind::TimedOut => "timed out",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Unauthenticated => "unauthenticated",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Internal => "internal",
            ErrorKind::Other => "other",
        };
        f.write_str(name)
    }
}

/// An owned application error with a context chain, a [`kind`](Error::kind)
/// and lossless access to the underlying causes.
///
/// An `Error` is either a leaf message ([`Error::msg`],
/// [`Error::with_kind`]), a transparent wrapper around a foreign
/// [`std::error::Error`] ([`Error::new`]), or a context layer around another
/// `Error` ([`Error::context`], [`Error::with_context`]). Layers form a
/// chain: [`Error::chain`] walks it, [`Error::root_cause`] finds its base and
/// [`Error::downcast_ref`] recovers typed causes.
///
/// ```
/// use akku::error::Error;
///
/// let error = Error::msg("root").context("middle").context("outer");
/// assert_eq!(error.to_string(), "outer");
/// assert_eq!(format!("{:#}", error), "outer: middle: root");
/// ```
pub struct Error {
    inner: Box<Inner>,
}

/// Heap-allocated identity of one error value; keeps [`Error`] a thin pointer.
struct Inner {
    kind: ErrorKind,
    repr: Repr,
}

/// How this error node stores its payload.
enum Repr {
    /// A leaf error carrying only a message.
    Msg { message: String },
    /// A transparent wrapper around a foreign error; the wrapper's display
    /// message *is* the wrapped error's message, so the wrapper never adds a
    /// duplicate layer to the chain.
    Source(Box<dyn std::error::Error + Send + Sync + 'static>),
    /// A context layer around another `Error`.
    Context { message: String, source: Error },
}

impl Error {
    /// Creates an error from a plain message, classified as
    /// [`ErrorKind::Other`].
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let error = Error::msg("something failed");
    /// assert_eq!(error.to_string(), "something failed");
    /// ```
    pub fn msg(message: impl Into<String>) -> Self {
        Self {
            inner: Box::new(Inner {
                kind: ErrorKind::Other,
                repr: Repr::Msg {
                    message: message.into(),
                },
            }),
        }
    }

    /// Wraps a foreign error (for example `std::io::Error`) losslessly.
    ///
    /// The source stays readable via `source()`, [`Error::chain`],
    /// [`Error::root_cause`] and [`Error::downcast_ref`].
    ///
    /// Wrapping an [`Error`] in an [`Error`] is a no-op: the value is returned
    /// unchanged. That keeps its [`ErrorKind`], never double-boxes it and makes
    /// [`Context`] on an `akku::error::Result` preserve the kind.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let error = Error::new(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke"));
    /// assert_eq!(error.to_string(), "pipe broke");
    /// assert!(error.is::<std::io::Error>());
    /// ```
    pub fn new<E>(source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        // An `akku::error::Error` is already a complete error value: return it
        // unchanged instead of wrapping it in a second transparent layer. The
        // `Any` probe costs one vtable comparison and needs no specialization.
        let mut slot = Some(source);
        if let Some(existing) =
            (&mut slot as &mut dyn std::any::Any).downcast_mut::<Option<Error>>()
        {
            return existing.take().expect("slot holds the error");
        }
        let source = slot.expect("slot holds the error");
        Self {
            inner: Box::new(Inner {
                kind: ErrorKind::Other,
                repr: Repr::Source(Box::new(source)),
            }),
        }
    }

    /// Creates an error from a message with an explicit [`ErrorKind`].
    ///
    /// ```
    /// use akku::error::{Error, ErrorKind};
    ///
    /// let error = Error::with_kind(ErrorKind::NotFound, "no such user");
    /// assert_eq!(error.kind(), ErrorKind::NotFound);
    /// ```
    pub fn with_kind(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            inner: Box::new(Inner {
                kind,
                repr: Repr::Msg {
                    message: message.into(),
                },
            }),
        }
    }

    /// Returns the structured kind of this error.
    ///
    /// Context layers preserve the kind of the error they wrap.
    ///
    /// ```
    /// use akku::error::{Error, ErrorKind};
    ///
    /// let error = Error::with_kind(ErrorKind::TimedOut, "slow").context("waiting for db");
    /// assert_eq!(error.kind(), ErrorKind::TimedOut);
    /// ```
    pub fn kind(&self) -> ErrorKind {
        self.inner.kind
    }

    /// Wraps this error with an additional context message on top.
    ///
    /// The full chain of the wrapped error is preserved.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let error = Error::msg("root").context("outer");
    /// assert_eq!(error.to_string(), "outer");
    /// assert_eq!(format!("{:#}", error), "outer: root");
    /// ```
    pub fn context(self, message: impl Into<String>) -> Self {
        self.with_context(|| message)
    }

    /// Wraps this error with a lazily built context message on top.
    ///
    /// The closure runs exactly once, when the layer is constructed.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let error = Error::msg("root").with_context(|| format!("layer for {}", 1 + 1));
    /// assert_eq!(error.to_string(), "layer for 2");
    /// ```
    pub fn with_context<F, S>(self, f: F) -> Self
    where
        F: FnOnce() -> S,
        S: Into<String>,
    {
        let kind = self.inner.kind;
        Self {
            inner: Box::new(Inner {
                kind,
                repr: Repr::Context {
                    message: f().into(),
                    source: self,
                },
            }),
        }
    }

    /// Iterates over the full error chain, outermost layer first.
    ///
    /// Transparent source wrappers (see [`Error::new`]) are skipped, so no
    /// layer appears twice.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let error = Error::msg("root").context("outer");
    /// let layers: Vec<String> = error.chain().map(|layer| layer.to_string()).collect();
    /// assert_eq!(layers, ["outer", "root"]);
    /// ```
    pub fn chain(&self) -> Chain<'_> {
        Chain::new(self)
    }

    /// Returns the deepest error in the chain that started this failure.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let error = Error::msg("root").context("outer");
    /// assert_eq!(error.root_cause().to_string(), "root");
    /// ```
    pub fn root_cause(&self) -> &(dyn std::error::Error + 'static) {
        let mut chain = self.chain();
        let mut last = chain
            .next()
            .expect("an error chain always has at least one layer");
        for layer in chain {
            last = layer;
        }
        last
    }

    /// Walks the whole chain looking for a cause of type `T`.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let source = "x".parse::<u32>().unwrap_err();
    /// let error = Error::new(source).context("parsing count");
    /// assert!(error.downcast_ref::<std::num::ParseIntError>().is_some());
    /// ```
    pub fn downcast_ref<T>(&self) -> Option<&T>
    where
        T: std::error::Error + 'static,
    {
        self.chain().find_map(|layer| layer.downcast_ref::<T>())
    }

    /// Returns whether any layer of the chain is of type `T`.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let error = Error::new(std::fmt::Error).context("formatting");
    /// assert!(error.is::<std::fmt::Error>());
    /// ```
    pub fn is<T>(&self) -> bool
    where
        T: std::error::Error + 'static,
    {
        self.downcast_ref::<T>().is_some()
    }
}

/// Iterator over an [`Error`]'s chain; created by [`Error::chain`].
///
/// Each item is one layer as `&(dyn std::error::Error + 'static)`, ordered
/// outermost first.
pub struct Chain<'a> {
    state: ChainStep<'a>,
}

/// Traversal position of [`Chain`].
enum ChainStep<'a> {
    /// The next yielded item is this `akku::error::Error` node.
    Node(&'a Error),
    /// The next yielded item is this foreign error node.
    Foreign(&'a (dyn std::error::Error + 'static)),
    /// The chain is exhausted.
    Done,
}

impl<'a> Chain<'a> {
    fn new(error: &'a Error) -> Self {
        Self {
            state: Self::enter_error(error),
        }
    }

    /// Enters an `Error` node, skipping transparent source wrappers.
    fn enter_error(error: &'a Error) -> ChainStep<'a> {
        match &error.inner.repr {
            Repr::Source(source) => ChainStep::Foreign(&**source),
            _ => ChainStep::Node(error),
        }
    }

    /// Continues after a yielded `Error` node.
    fn advance_node(error: &'a Error) -> ChainStep<'a> {
        match &error.inner.repr {
            Repr::Msg { .. } => ChainStep::Done,
            Repr::Source(source) => ChainStep::Foreign(&**source),
            Repr::Context { source, .. } => Self::enter_error(source),
        }
    }

    /// Continues after a yielded foreign error node, re-injecting
    /// `akku::error` nodes (which may appear when a foreign error's own source
    /// chain holds an [`Error`], for example a boxed one).
    fn advance_foreign(error: &'a (dyn std::error::Error + 'static)) -> ChainStep<'a> {
        match error.source() {
            None => ChainStep::Done,
            Some(source) => match source.downcast_ref::<Error>() {
                Some(inner) => Self::enter_error(inner),
                None => ChainStep::Foreign(source),
            },
        }
    }
}

impl<'a> Iterator for Chain<'a> {
    type Item = &'a (dyn std::error::Error + 'static);

    fn next(&mut self) -> Option<Self::Item> {
        match std::mem::replace(&mut self.state, ChainStep::Done) {
            ChainStep::Done => None,
            ChainStep::Node(error) => {
                self.state = Self::advance_node(error);
                Some(error)
            }
            ChainStep::Foreign(error) => {
                self.state = Self::advance_foreign(error);
                Some(error)
            }
        }
    }
}

impl std::fmt::Display for Error {
    /// Prints only the top message of the chain.
    ///
    /// The alternate form `{:#}` joins every layer:
    /// `"outer: middle: root"`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !f.alternate() {
            return self.write_top_message(f);
        }
        let mut first = true;
        for layer in self.chain() {
            if !first {
                f.write_str(": ")?;
            }
            first = false;
            // Our own nodes must be written without re-entering this
            // alternate branch, which would recurse forever.
            match layer.downcast_ref::<Error>() {
                Some(error) => error.write_top_message(f)?,
                None => std::fmt::Display::fmt(layer, f)?,
            }
        }
        Ok(())
    }
}

impl Error {
    /// Writes this node's own display message (never the whole chain).
    fn write_top_message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner.repr {
            Repr::Msg { message } | Repr::Context { message, .. } => f.write_str(message),
            Repr::Source(source) => std::fmt::Display::fmt(&**source, f),
        }
    }
}

impl std::fmt::Debug for Error {
    /// Renders the top message plus every cause, similar to `anyhow`'s
    /// debug output.
    ///
    /// ```
    /// use akku::error::Error;
    ///
    /// let debug = format!("{:?}", Error::msg("root").context("outer"));
    /// assert!(debug.contains("outer"));
    /// assert!(debug.contains("root"));
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)?;
        let causes: Vec<&(dyn std::error::Error + 'static)> = self.chain().skip(1).collect();
        if causes.is_empty() {
            return Ok(());
        }
        write!(f, "\n\nCaused by:")?;
        if let [only] = causes[..] {
            write!(f, "\n    {only}")?;
        } else {
            for (index, cause) in causes.iter().enumerate() {
                write!(f, "\n    {index}: {cause}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    /// The next layer of the chain, with transparent source wrappers
    /// skipped so that walking `source()` never repeats a layer.
    ///
    /// A transparent source wrapper displays exactly its source's message, so it
    /// yields that source's own source instead of the source itself.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.inner.repr {
            Repr::Msg { .. } => None,
            Repr::Source(source) => source.source(),
            Repr::Context { source, .. } => Some(match &source.inner.repr {
                Repr::Source(inner) => &**inner,
                _ => source,
            }),
        }
    }
}

/// Converts from `std::io::Error` so that `?` works in functions returning
/// [`Result`].
impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::new(source)
    }
}

/// Converts from `std::fmt::Error`.
impl From<std::fmt::Error> for Error {
    fn from(source: std::fmt::Error) -> Self {
        Self::new(source)
    }
}

/// Converts from `std::str::Utf8Error`.
impl From<std::str::Utf8Error> for Error {
    fn from(source: std::str::Utf8Error) -> Self {
        Self::new(source)
    }
}

/// Converts from `std::string::FromUtf8Error`.
impl From<std::string::FromUtf8Error> for Error {
    fn from(source: std::string::FromUtf8Error) -> Self {
        Self::new(source)
    }
}

/// Converts from `std::num::ParseIntError`.
impl From<std::num::ParseIntError> for Error {
    fn from(source: std::num::ParseIntError) -> Self {
        Self::new(source)
    }
}

/// Converts from `std::num::ParseFloatError`.
impl From<std::num::ParseFloatError> for Error {
    fn from(source: std::num::ParseFloatError) -> Self {
        Self::new(source)
    }
}

/// Adds [`Error`]-context to fallible values.
///
/// Implemented for `Result<T, E>` where `E: std::error::Error + Send + Sync +
/// 'static`, and for `Option<T>`.
///
/// ```
/// use akku::error::Context;
///
/// let value: Option<u8> = None;
/// assert_eq!(value.context("missing byte").unwrap_err().to_string(), "missing byte");
/// ```
pub trait Context<T> {
    /// Wraps the error (or provides one for a `None`) with `context` as the
    /// new top message.
    fn context<C: Into<String>>(self, context: C) -> Result<T>;

    /// Like [`Context::context`], but the message is built by `f` only when
    /// an error is actually present.
    fn with_context<C: Into<String>, F: FnOnce() -> C>(self, f: F) -> Result<T>;
}

impl<T, E> Context<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn context<C: Into<String>>(self, context: C) -> Result<T> {
        self.map_err(|error| Error::new(error).context(context))
    }

    fn with_context<C: Into<String>, F: FnOnce() -> C>(self, f: F) -> Result<T> {
        self.map_err(|error| Error::new(error).context(f()))
    }
}

impl<T> Context<T> for Option<T> {
    fn context<C: Into<String>>(self, context: C) -> Result<T> {
        self.ok_or_else(|| Error::msg(context))
    }

    fn with_context<C: Into<String>, F: FnOnce() -> C>(self, f: F) -> Result<T> {
        self.ok_or_else(|| Error::msg(f()))
    }
}

/// Implementation detail behind [`bail`]; do not use directly.
#[doc(hidden)]
#[macro_export]
macro_rules! __akku_bail {
    ($($arg:tt)+) => {
        return ::core::result::Result::Err(::core::convert::From::from(
            $crate::error::Error::msg(::std::format!($($arg)+)),
        ))
    };
}

/// Implementation detail behind [`ensure`]; do not use directly.
#[doc(hidden)]
#[macro_export]
macro_rules! __akku_ensure {
    ($cond:expr) => {
        $crate::__akku_ensure!($cond, concat!("condition failed: ", stringify!($cond)))
    };
    ($cond:expr,) => {
        $crate::__akku_ensure!($cond)
    };
    ($cond:expr, $($arg:tt)+) => {
        if !($cond) {
            $crate::__akku_bail!($($arg)+);
        }
    };
}

/// Early-returns from the enclosing function with an
/// [`Error`] built from a formatted message.
///
/// Works in any function returning a `Result` whose error type converts from
/// [`Error`].
///
/// ```
/// use akku::error::{bail, Result};
///
/// fn clamp(count: i64) -> Result<i64> {
///     if count < 0 {
///         bail!("count must not be negative, got {count}");
///     }
///     Ok(count)
/// }
///
/// assert_eq!(clamp(-3).unwrap_err().to_string(), "count must not be negative, got -3");
/// ```
#[doc(inline)]
pub use crate::__akku_bail as bail;

/// Returns with an [`Error`] unless `$cond` is true.
///
/// With only a condition, the message is `condition failed: <condition>`;
/// with extra arguments it formats like `format!`.
///
/// ```
/// use akku::error::{ensure, Result};
///
/// fn divide(dividend: f64, divisor: f64) -> Result<f64> {
///     ensure!(divisor != 0.0, "cannot divide by zero");
///     Ok(dividend / divisor)
/// }
///
/// assert!(divide(1.0, 0.0).unwrap_err().to_string().contains("divide by zero"));
/// ```
#[doc(inline)]
pub use crate::__akku_ensure as ensure;
