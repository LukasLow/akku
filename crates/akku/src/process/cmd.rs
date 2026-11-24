//! Single-command construction and execution.

use std::borrow::Cow;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::io::Read as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

/// How much stderr [`CmdError`] prints before truncating.
const STDERR_DISPLAY_LIMIT: usize = 4096;

/// One environment mutation recorded for a [`Cmd`].
#[derive(Debug, Clone)]
enum EnvOp {
    /// `env(key, value)`.
    Set(OsString, OsString),
    /// `env_remove(key)`.
    Remove(OsString),
    /// `env_clear()`.
    Clear,
}

/// The captured result of running a [`Cmd`] to completion.
///
/// # Examples
///
/// ```
/// use akku::process::Cmd;
/// # // These examples use Unix tools (`echo`, `tr`, `sort`, `printf`).
/// # #[cfg(unix)] {
///
/// let out = Cmd::new("echo").arg("hi").run().unwrap();
/// assert!(out.success());
/// assert_eq!(out.stdout_str().unwrap(), "hi\n");
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct CmdOutput {
    /// How the process exited.
    pub status: ExitStatus,
    /// Everything the process wrote to stdout.
    pub stdout: Vec<u8>,
    /// Everything the process wrote to stderr.
    pub stderr: Vec<u8>,
    program: String,
}

impl CmdOutput {
    /// Build an output value directly; its `program` name stays empty.
    pub fn new(status: ExitStatus, stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self {
            status,
            stdout,
            stderr,
            program: String::new(),
        }
    }

    /// Whether the process exited successfully.
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// The whole stdout as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns the UTF-8 error when stdout is not valid UTF-8.
    pub fn stdout_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.stdout)
    }

    /// The whole stderr as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns the UTF-8 error when stderr is not valid UTF-8.
    pub fn stderr_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.stderr)
    }

    /// The whole stdout as lossy UTF-8; invalid bytes become U+FFFD.
    pub fn stdout_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    /// The whole stderr as lossy UTF-8; invalid bytes become U+FFFD.
    pub fn stderr_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }

    /// Check the exit status, describing failures with [`CmdError`].
    ///
    /// # Errors
    ///
    /// Returns the program name and the captured output for every
    /// non-success status.
    pub fn ensure_success(&self) -> Result<&Self, CmdError> {
        if self.status.success() {
            return Ok(self);
        }
        Err(CmdError {
            program: self.program.clone(),
            output: self.clone(),
        })
    }
}

/// A failed command, carrying the program name and its captured output.
///
/// The `Display` implementation prints the exit status plus up to 4096 bytes
/// of stderr, truncating on a UTF-8 boundary.
#[derive(Debug, Clone)]
pub struct CmdError {
    /// The program whose execution failed.
    pub program: String,
    /// The captured output of the failed run.
    pub output: CmdOutput,
}

impl fmt::Display for CmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "command `{}` failed: {}",
            self.program, self.output.status
        )?;
        let bytes = &self.output.stderr;
        if bytes.is_empty() {
            return Ok(());
        }
        let end = truncate_utf8_boundary(bytes, STDERR_DISPLAY_LIMIT);
        let truncated = end < bytes.len();
        let text = String::from_utf8_lossy(&bytes[..end]);
        if truncated {
            write!(f, "; stderr: {text}...")
        } else {
            write!(f, "; stderr: {text}")
        }
    }
}

/// Adjusts `limit` down by at most 3 bytes if `limit < bytes.len()` and
/// `bytes[limit]` is inside a valid multi-byte UTF-8 sequence.
fn truncate_utf8_boundary(bytes: &[u8], limit: usize) -> usize {
    let end = bytes.len().min(limit);
    if end == bytes.len() || end == 0 {
        return end;
    }
    if (bytes[end] & 0xC0) != 0x80 {
        return end;
    }
    let min_start = end.saturating_sub(3);
    let mut pos = end - 1;
    while pos > min_start && (bytes[pos] & 0xC0) == 0x80 {
        pos -= 1;
    }
    let width = match bytes[pos] {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return end,
    };
    if pos + width > end
        && pos + width <= bytes.len()
        && std::str::from_utf8(&bytes[pos..pos + width]).is_ok()
    {
        pos
    } else {
        end
    }
}

impl Error for CmdError {}

/// A command under construction, mirroring [`std::process::Command`] and
/// adding stdin payload support.
///
/// A `Cmd` is cheap to clone; every [`run`](Cmd::run) call rebuilds an
/// equivalent std [`Command`], so the same builder can be run and rerun.
///
/// # Examples
///
/// ```
/// use akku::process::Cmd;
/// # // These examples use Unix tools (`echo`, `tr`, `sort`, `printf`).
/// # #[cfg(unix)] {
///
/// let out = Cmd::new("tr").arg("a-z").arg("A-Z")
///     .stdin_bytes(b"shh".as_slice())
///     .run()
///     .unwrap();
/// assert_eq!(out.stdout_str().unwrap(), "SHH");
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Cmd {
    program: OsString,
    args: Vec<OsString>,
    dir: Option<PathBuf>,
    env_ops: Vec<EnvOp>,
    stdin_bytes: Option<Vec<u8>>,
}

impl Cmd {
    /// Start building a run of `program`.
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            dir: None,
            env_ops: Vec::new(),
            stdin_bytes: None,
        }
    }

    /// Append one argument.
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Append several arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    /// Run the command inside a different working directory.
    pub fn dir(mut self, dir: impl AsRef<std::path::Path>) -> Self {
        self.dir = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Set one environment variable for the child process.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env_ops.push(EnvOp::Set(
            key.as_ref().to_owned(),
            value.as_ref().to_owned(),
        ));
        self
    }

    /// Set several environment variables at once.
    pub fn envs<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.env_ops
            .extend(vars.into_iter().map(|(key, value)| {
                EnvOp::Set(key.as_ref().to_owned(), value.as_ref().to_owned())
            }));
        self
    }

    /// Unset one variable for the child process.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.env_ops.push(EnvOp::Remove(key.as_ref().to_owned()));
        self
    }

    /// Clear every environment variable for the child process.
    pub fn env_clear(mut self) -> Self {
        self.env_ops.push(EnvOp::Clear);
        self
    }

    /// Feed `bytes` into the child's stdin.
    ///
    /// The payload is written by a background thread so that even payloads
    /// larger than a pipe buffer cannot deadlock the run; [`Cmd::status`]
    /// runs with inherited standard streams and ignores any payload.
    pub fn stdin_bytes(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin_bytes = Some(bytes.into());
        self
    }

    /// The equivalent std [`Command`], freshly configured.
    ///
    /// Settings this builder does not cover can be applied to the returned
    /// command directly.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if let Some(dir) = &self.dir {
            command.current_dir(dir);
        }
        for op in &self.env_ops {
            match op {
                EnvOp::Set(key, value) => {
                    command.env(key, value);
                }
                EnvOp::Remove(key) => {
                    command.env_remove(key);
                }
                EnvOp::Clear => {
                    command.env_clear();
                }
            }
        }
        command
    }

    /// The program this command was built from, losslessly.
    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// The program this command was built from, as display text.
    pub fn program_name(&self) -> String {
        self.program.to_string_lossy().into_owned()
    }

    /// Whether a stdin payload was configured.
    pub(crate) fn has_stdin_bytes(&self) -> bool {
        self.stdin_bytes.is_some()
    }

    /// The configured stdin payload, if any.
    pub(crate) fn stdin_payload(&self) -> Option<&[u8]> {
        self.stdin_bytes.as_deref()
    }

    /// Run the command with stdout and stderr captured.
    ///
    /// Without a [`stdin_bytes`](Cmd::stdin_bytes) payload the child's stdin
    /// is a null pipe: reads from it observe an immediate EOF. Stderr is
    /// drained on a background thread, so children that emit more stderr
    /// than a pipe buffer holds cannot wedge the run.
    ///
    /// # Errors
    ///
    /// Propagates any I/O error from spawning the child or reading its
    /// streams.
    pub fn run(&self) -> io::Result<CmdOutput> {
        let mut command = self.command();
        let payload = if self.stdin_bytes.is_some() {
            command.stdin(Stdio::piped());
            self.stdin_bytes.clone()
        } else {
            command.stdin(Stdio::null());
            None
        };
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut child = command.spawn()?;

        let stderr_reader = child.stderr.take().map(|mut handle| {
            thread::spawn(move || {
                let mut buffer = Vec::new();
                handle.read_to_end(&mut buffer)?;
                Ok::<Vec<u8>, io::Error>(buffer)
            })
        });

        let stdin_writer = if let (Some(bytes), Some(mut stdin)) = (payload, child.stdin.take()) {
            Some(thread::spawn(move || {
                // The child may exit early with a closed pipe; ignore.
                let _ = stdin.write_all(&bytes);
            }))
        } else {
            None
        };

        let mut stdout = Vec::new();
        if let Some(mut handle) = child.stdout.take() {
            if let Err(err) = handle.read_to_end(&mut stdout) {
                drop(handle);
                let _ = child.kill();
                let _ = child.wait();
                if let Some(reader) = stderr_reader {
                    let _ = reader.join();
                }
                if let Some(writer) = stdin_writer {
                    let _ = writer.join();
                }
                return Err(err);
            }
        }

        let stderr = match stderr_reader.map(|handle| handle.join()) {
            Some(Ok(Ok(bytes))) => bytes,
            Some(Ok(Err(err))) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(writer) = stdin_writer {
                    let _ = writer.join();
                }
                return Err(err);
            }
            Some(Err(_)) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(writer) = stdin_writer {
                    let _ = writer.join();
                }
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "stderr reader thread panicked",
                ));
            }
            None => Vec::new(),
        };

        let status = match child.wait() {
            Ok(status) => status,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(writer) = stdin_writer {
                    let _ = writer.join();
                }
                return Err(err);
            }
        };
        if let Some(writer) = stdin_writer {
            let _ = writer.join();
        }
        Ok(CmdOutput {
            status,
            stdout,
            stderr,
            program: self.program_name(),
        })
    }

    /// Run the command with all three standard streams inherited.
    ///
    /// # Errors
    ///
    /// Propagates any I/O error from spawning the child.
    pub fn status(&self) -> io::Result<ExitStatus> {
        self.command().status()
    }
}
