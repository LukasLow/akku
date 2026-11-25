//! Process execution: a command builder, pipelines, and `std` re-exports.
//!
//! The `process` domain bundles the `std::process` API with [`Cmd`], a
//! small builder that can feed a command its stdin bytes, and [`Pipeline`],
//! which runs several [`Cmd`] stages connected like shell pipes without the
//! classic capture deadlocks: every stage is spawned immediately and every
//! stderr/stdout stream is drained concurrently.
//!
//! # Examples
//!
//! ```
//! use akku::process::{Cmd, Pipeline};
//! # // These examples use Unix tools (`echo`, `tr`, `sort`, `printf`).
//! # #[cfg(unix)] {
//!
//! let out = Cmd::new("echo").arg("hello from akku").run().unwrap();
//! assert!(out.success());
//! assert_eq!(out.stdout_lossy().trim(), "hello from akku");
//!
//! let piped = Pipeline::new()
//!     .cmd(Cmd::new("echo").arg("b c a"))
//!     .cmd(Cmd::new("tr").arg(" ").arg("\n"))
//!     .cmd(Cmd::new("sort"))
//!     .run()
//!     .unwrap();
//! assert_eq!(piped.stdout, b"a\nb\nc\n");
//! # }
//! ```

pub(crate) mod cmd;
pub(crate) mod pipeline;

pub use cmd::{Cmd, CmdError, CmdOutput};
pub use pipeline::{Pipeline, PipelineOutput};

// The full stable std::process surface, re-exported explicitly.
pub use std::process::{
    abort, exit, id, Child, ChildStderr, ChildStdin, ChildStdout, Command, CommandArgs,
    CommandEnvs, ExitCode, ExitStatus, Output, Stdio, Termination,
};
