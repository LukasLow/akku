//! Multi-stage pipelines connected like shell pipes.

use std::borrow::Cow;
use std::io;
use std::io::Read as _;
use std::io::Write as _;
use std::process::{ChildStdout, ExitStatus, Stdio};
use std::str::Utf8Error;
use std::thread;

use super::cmd::{Cmd, CmdError, CmdOutput};

/// The captured result of a whole [`Pipeline`] run.
///
/// # Examples
///
/// ```
/// use akku::process::{Cmd, Pipeline};
/// # // These examples use Unix tools (`echo`, `tr`, `sort`, `printf`).
/// # #[cfg(unix)] {
///
/// let out = Pipeline::new()
///     .cmd(Cmd::new("printf").arg("b\\na\\n"))
///     .cmd(Cmd::new("sort"))
///     .run()
///     .unwrap();
/// assert!(out.success());
/// assert_eq!(out.stdout_str().unwrap(), "a\nb\n");
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PipelineOutput {
    /// Exit status of the last stage.
    pub status: ExitStatus,
    /// Exit status of every stage, in pipeline order.
    pub statuses: Vec<ExitStatus>,
    /// Stdout of the last stage.
    pub stdout: Vec<u8>,
    /// Stderr of every stage, in pipeline order: one buffer per stage.
    pub stderr: Vec<Vec<u8>>,
    programs: Vec<String>,
}

impl PipelineOutput {
    /// Whether the last stage exited successfully.
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// Whether every stage exited successfully.
    pub fn all_success(&self) -> bool {
        self.statuses.iter().all(ExitStatus::success)
    }

    /// The last stage's stdout as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns the UTF-8 error when stdout is not valid UTF-8.
    pub fn stdout_str(&self) -> Result<&str, Utf8Error> {
        std::str::from_utf8(&self.stdout)
    }

    /// Every stage's stderr as UTF-8, one entry per stage.
    pub fn stderr_str(&self) -> Vec<Result<&str, Utf8Error>> {
        self.stderr
            .iter()
            .map(|chunk| std::str::from_utf8(chunk))
            .collect()
    }

    /// The last stage's stdout as lossy UTF-8.
    pub fn stdout_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    /// Every stage's stderr as lossy UTF-8, one entry per stage.
    pub fn stderr_lossy(&self) -> Vec<String> {
        self.stderr
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect()
    }

    /// Check the last stage, describing failures with [`CmdError`].
    ///
    /// # Errors
    ///
    /// Returns the failed stage's program and captured output.
    pub fn ensure_success(&self) -> Result<&Self, CmdError> {
        if !self.status.success() {
            let program = self.programs.last().cloned().unwrap_or_default();
            return Err(CmdError {
                program,
                output: CmdOutput::new(
                    self.status,
                    self.stdout.clone(),
                    self.stderr.last().cloned().unwrap_or_default(),
                ),
            });
        }
        Ok(self)
    }

    /// Check every stage, describing the first failure with [`CmdError`].
    ///
    /// # Errors
    ///
    /// Returns the first failing stage's program and captured output.
    pub fn ensure_all_success(&self) -> Result<&Self, CmdError> {
        for (index, status) in self.statuses.iter().enumerate() {
            if !status.success() {
                return Err(CmdError {
                    program: self.programs.get(index).cloned().unwrap_or_default(),
                    output: CmdOutput::new(
                        *status,
                        Vec::new(),
                        self.stderr.get(index).cloned().unwrap_or_default(),
                    ),
                });
            }
        }
        Ok(self)
    }
}

/// One or more [`Cmd`] stages connected like a shell pipeline.
///
/// Stages are spawned immediately one after the other (their pipes connect
/// straight into the next stage) and all captured streams are drained
/// concurrently, so pipelines with more traffic than a pipe buffer holds
/// never deadlock. A [`Pipeline`] is reusable: [`run`](Pipeline::run) can be
/// called any number of times.
///
/// # Examples
///
/// ```
/// use akku::process::{Cmd, Pipeline};
/// # // These examples use Unix tools (`echo`, `tr`, `sort`, `printf`).
/// # #[cfg(unix)] {
///
/// let out = Pipeline::new()
///     .cmd(Cmd::new("echo").arg("one two"))
///     .cmd(Cmd::new("tr").arg(" ").arg("\n"))
///     .run()
///     .unwrap();
/// assert_eq!(out.stdout_lossy(), "one\ntwo\n");
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Pipeline {
    stages: Vec<Cmd>,
}

impl Pipeline {
    /// An empty pipeline.
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Append one stage.
    pub fn cmd(mut self, cmd: Cmd) -> Self {
        self.stages.push(cmd);
        self
    }

    /// How many stages the pipeline has.
    pub fn len(&self) -> usize {
        self.stages.len()
    }

    /// Whether the pipeline has no stages.
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Run the pipeline, capturing the last stage's stdout and every
    /// stage's stderr.
    ///
    /// The first stage's stdin: a [`stdin_bytes`](Cmd::stdin_bytes) payload
    /// when configured, else an immediate EOF. Any later stage with a stdin
    /// payload is a configuration error.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for an empty pipeline or a
    /// non-first stage with a stdin payload, and propagates any I/O error
    /// from spawning or reading.
    pub fn run(&self) -> io::Result<PipelineOutput> {
        if self.stages.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pipeline has no stages",
            ));
        }
        for (index, stage) in self.stages.iter().enumerate() {
            if index > 0 && stage.has_stdin_bytes() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "stage {index} carries a stdin payload, but only the first stage may read stdin"
                    ),
                ));
            }
        }

        let last = self.stages.len() - 1;
        let mut children = Vec::with_capacity(self.stages.len());
        let mut stderr_readers: Vec<thread::JoinHandle<io::Result<Vec<u8>>>> =
            Vec::with_capacity(self.stages.len());
        let mut stdin_writer: Option<thread::JoinHandle<()>> = None;
        let mut previous_stdout: Option<ChildStdout> = None;

        for (index, stage) in self.stages.iter().enumerate() {
            let mut command = stage.command();
            if index == 0 {
                // Always a pipe: the payload (if any) is written by a
                // thread, then EOF arrives when the handle is dropped.
                command.stdin(Stdio::piped());
            } else {
                let feed = previous_stdout.take().expect("previous stage stdout");
                command.stdin(Stdio::from(feed));
            }
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());

            match command.spawn() {
                Ok(child) => {
                    children.push(child);
                }
                Err(err) => {
                    drop(previous_stdout.take());
                    cleanup_pipeline(children, stderr_readers, stdin_writer);
                    return Err(err);
                }
            }

            let child = children.last_mut().expect("just pushed");
            if index == 0 {
                if let Some(mut stdin) = child.stdin.take() {
                    if let Some(payload) = stage.stdin_payload() {
                        let payload = payload.to_vec();
                        stdin_writer = Some(thread::spawn(move || {
                            let _ = stdin.write_all(&payload);
                        }));
                    } else {
                        // Dropping the handle closes the child's stdin.
                        drop(stdin);
                    }
                }
            }
            if let Some(mut handle) = child.stderr.take() {
                stderr_readers.push(thread::spawn(move || {
                    let mut buffer = Vec::new();
                    handle.read_to_end(&mut buffer)?;
                    Ok(buffer)
                }));
            }
            previous_stdout = child.stdout.take();
        }

        // Drain the last stage's stdout while it still runs.
        let mut stdout = Vec::new();
        if let Some(mut handle) = previous_stdout.take() {
            if let Err(err) = handle.read_to_end(&mut stdout) {
                drop(handle);
                cleanup_pipeline(children, stderr_readers, stdin_writer);
                return Err(err);
            }
        }

        let mut stderr = Vec::with_capacity(stderr_readers.len());
        let mut reader_iter = stderr_readers.into_iter();
        while let Some(reader) = reader_iter.next() {
            let result = match reader.join() {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(err)) => {
                    let remaining: Vec<_> = reader_iter.collect();
                    cleanup_pipeline(children, remaining, stdin_writer);
                    return Err(err);
                }
                Err(_) => {
                    let remaining: Vec<_> = reader_iter.collect();
                    cleanup_pipeline(children, remaining, stdin_writer);
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "stderr reader thread panicked",
                    ));
                }
            };
            stderr.push(result);
        }

        let mut statuses = Vec::with_capacity(children.len());
        let mut child_iter = children.into_iter();
        while let Some(mut child) = child_iter.next() {
            match child.wait() {
                Ok(status) => statuses.push(status),
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let remaining: Vec<_> = child_iter.collect();
                    cleanup_pipeline(remaining, Vec::new(), stdin_writer);
                    return Err(err);
                }
            }
        }
        if let Some(writer) = stdin_writer {
            let _ = writer.join();
        }

        let programs = self
            .stages
            .iter()
            .map(|stage| stage.program_name())
            .collect();
        let status = statuses[last];
        Ok(PipelineOutput {
            status,
            statuses,
            stdout,
            stderr,
            programs,
        })
    }
}

fn cleanup_pipeline(
    children: Vec<std::process::Child>,
    stderr_readers: Vec<thread::JoinHandle<io::Result<Vec<u8>>>>,
    stdin_writer: Option<thread::JoinHandle<()>>,
) {
    for mut child in children {
        let _ = child.kill();
        let _ = child.wait();
    }
    for reader in stderr_readers {
        let _ = reader.join();
    }
    if let Some(writer) = stdin_writer {
        let _ = writer.join();
    }
}
