//! Testing utilities: temporary directories, snapshot testing with line diffs,
//! and (with the `http` feature) an RAII in-process HTTP test server and client.
//!
//! # Examples
//!
//! ```
//! use akku::test::{Snapshot, TempDir};
//!
//! let dir = TempDir::new().unwrap();
//! dir.write("nested/hello.txt", "world").unwrap();
//! assert_eq!(dir.read_to_string("nested/hello.txt").unwrap(), "world");
//!
//! let snap_path = dir.child("greeting.snap");
//! Snapshot::at(&snap_path).update(true).assert_eq("hello\nworld\n");
//! Snapshot::at(&snap_path).assert_eq("hello\nworld\n");
//! ```

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// TempDir
// ---------------------------------------------------------------------------

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// An automatically cleaned-up temporary directory created in [`std::env::temp_dir`].
///
/// Unless [`TempDir::persist`] is called, the directory and all its contents are
/// removed when the `TempDir` is dropped (ignoring I/O errors during `Drop`).
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
    persist: bool,
}

impl TempDir {
    /// Creates a new uniquely named temporary directory with prefix `"akku-test"`.
    pub fn new() -> io::Result<Self> {
        Self::with_prefix("akku-test")
    }

    /// Creates a new uniquely named temporary directory using `prefix`.
    pub fn with_prefix(prefix: &str) -> io::Result<Self> {
        let base = std::env::temp_dir();
        let pid = std::process::id();

        for _ in 0..1024 {
            let seq = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let candidate = base.join(format!("{prefix}-{pid}-{seq}-{nanos}"));
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    return Ok(Self {
                        path: candidate,
                        persist: false,
                    });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "failed to create unique temporary directory after 1024 attempts",
        ))
    }

    /// Returns the path of the temporary directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Joins a relative path onto this temporary directory.
    pub fn child(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.path.join(rel)
    }

    /// Writes `contents` to `rel` inside this directory, creating parent directories as needed.
    pub fn write(&self, rel: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<PathBuf> {
        let target = self.child(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, contents)?;
        Ok(target)
    }

    /// Reads a UTF-8 file at `rel` inside this directory into a [`String`].
    pub fn read_to_string(&self, rel: impl AsRef<Path>) -> io::Result<String> {
        fs::read_to_string(self.child(rel))
    }

    /// Consumes the `TempDir` without deleting it, returning its [`PathBuf`].
    pub fn persist(mut self) -> PathBuf {
        self.persist = true;
        self.path.clone()
    }

    /// Explicitly removes the temporary directory and returns any I/O error.
    pub fn close(mut self) -> io::Result<()> {
        self.persist = true;
        fs::remove_dir_all(&self.path)
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.persist {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot testing with LCS line diff
// ---------------------------------------------------------------------------

/// Difference reported when a [`Snapshot`] comparison fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDiff {
    path: PathBuf,
    summary: String,
}

impl SnapshotDiff {
    /// Returns the snapshot file path that failed comparison.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for SnapshotDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "snapshot mismatch at {}:\n{}",
            self.path.display(),
            self.summary
        )
    }
}

impl Error for SnapshotDiff {}

/// File-backed text snapshot verifier with LCS line diffing.
#[derive(Debug, Clone)]
pub struct Snapshot {
    path: PathBuf,
    update: bool,
    normalize_crlf: bool,
}

impl Snapshot {
    /// Creates a snapshot named `name` stored at `tests/snapshots/<name>.snap`
    /// (relative to `CARGO_MANIFEST_DIR` if set).
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        let base = std::env::var_os("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let path = base
            .join("tests")
            .join("snapshots")
            .join(format!("{name}.snap"));
        Self::at(path)
    }

    /// Creates a snapshot backed by the explicit file `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        let update = std::env::var("AKKU_UPDATE_SNAPSHOTS")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        Self {
            path: path.into(),
            update,
            normalize_crlf: true,
        }
    }

    /// Forces snapshot update mode on or off (writing `actual` to disk when `true`).
    pub fn update(mut self, yes: bool) -> Self {
        self.update = yes;
        self
    }

    /// Controls whether `\r\n` line endings are normalized to `\n` before comparison.
    pub fn normalize_crlf(mut self, yes: bool) -> Self {
        self.normalize_crlf = yes;
        self
    }

    /// Compares `actual` against the stored snapshot file (or updates it in update mode).
    pub fn compare(&self, actual: &str) -> Result<(), SnapshotDiff> {
        let actual_norm = if self.normalize_crlf {
            actual.replace("\r\n", "\n")
        } else {
            actual.to_string()
        };

        if self.update {
            if let Some(parent) = self.path.parent() {
                if let Err(err) = fs::create_dir_all(parent) {
                    return Err(SnapshotDiff {
                        path: self.path.clone(),
                        summary: format!("failed to create snapshot directory: {err}"),
                    });
                }
            }
            if let Err(err) = fs::write(&self.path, &actual_norm) {
                return Err(SnapshotDiff {
                    path: self.path.clone(),
                    summary: format!("failed to write snapshot file: {err}"),
                });
            }
            return Ok(());
        }

        let expected_raw = match fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(err) => {
                return Err(SnapshotDiff {
                    path: self.path.clone(),
                    summary: format!(
                        "snapshot file could not be read ({err}); set AKKU_UPDATE_SNAPSHOTS=1 to create it"
                    ),
                });
            }
        };

        let expected_norm = if self.normalize_crlf {
            expected_raw.replace("\r\n", "\n")
        } else {
            expected_raw
        };

        if expected_norm == actual_norm {
            return Ok(());
        }

        let diff_text = format_line_diff(&expected_norm, &actual_norm);
        Err(SnapshotDiff {
            path: self.path.clone(),
            summary: diff_text,
        })
    }

    /// Asserts that `actual` matches the snapshot, panicking with a readable diff on mismatch.
    #[track_caller]
    pub fn assert_eq(&self, actual: &str) {
        if let Err(diff) = self.compare(actual) {
            panic!("{diff}");
        }
    }
}

/// Asserts that `actual` matches the named snapshot (`tests/snapshots/<name>.snap`).
#[track_caller]
pub fn assert_snapshot(name: &str, actual: &str) {
    Snapshot::named(name).assert_eq(actual);
}

fn format_line_diff(expected: &str, actual: &str) -> String {
    let exp_lines: Vec<&str> = expected.lines().collect();
    let act_lines: Vec<&str> = actual.lines().collect();

    if exp_lines.len().max(act_lines.len()) > 2_000 {
        return format_linear_fallback_diff(&exp_lines, &act_lines);
    }

    // Classic DP LCS table: dp[i][j] = LCS length of exp_lines[i..] and act_lines[j..].
    let m = exp_lines.len();
    let n = act_lines.len();
    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            if exp_lines[i] == act_lines[j] {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    let mut out = String::new();
    let mut i = 0;
    let mut j = 0;
    while i < m && j < n {
        if exp_lines[i] == act_lines[j] {
            out.push_str("  ");
            out.push_str(exp_lines[i]);
            out.push('\n');
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push_str("- ");
            out.push_str(exp_lines[i]);
            out.push('\n');
            i += 1;
        } else {
            out.push_str("+ ");
            out.push_str(act_lines[j]);
            out.push('\n');
            j += 1;
        }
    }
    while i < m {
        out.push_str("- ");
        out.push_str(exp_lines[i]);
        out.push('\n');
        i += 1;
    }
    while j < n {
        out.push_str("+ ");
        out.push_str(act_lines[j]);
        out.push('\n');
        j += 1;
    }
    out
}

fn format_linear_fallback_diff(exp_lines: &[&str], act_lines: &[&str]) -> String {
    let mut out = String::new();
    let max_len = exp_lines.len().max(act_lines.len());
    for i in 0..max_len {
        match (exp_lines.get(i), act_lines.get(i)) {
            (Some(e), Some(a)) if e == a => {}
            (Some(e), Some(a)) => {
                out.push_str(&format!("line {}:\n- {e}\n+ {a}\n", i + 1));
            }
            (Some(e), None) => {
                out.push_str(&format!("line {}:\n- {e}\n", i + 1));
            }
            (None, Some(a)) => {
                out.push_str(&format!("line {}:\n+ {a}\n", i + 1));
            }
            (None, None) => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// HTTP TestServer & TestClient (behind `http` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "http")]
mod http_test {
    use std::future::Future;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::thread::{self, JoinHandle};

    use crate::http::App;

    struct ShutdownState {
        triggered: AtomicBool,
        waker: Mutex<Option<Waker>>,
    }

    impl ShutdownState {
        fn new() -> Self {
            Self {
                triggered: AtomicBool::new(false),
                waker: Mutex::new(None),
            }
        }

        fn trigger(&self) {
            self.triggered.store(true, Ordering::Release);
            if let Some(waker) = self.waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
                waker.wake();
            }
        }
    }

    struct ShutdownFuture {
        state: Arc<ShutdownState>,
    }

    impl Future for ShutdownFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.state.triggered.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            {
                let mut guard = self.state.waker.lock().unwrap_or_else(|e| e.into_inner());
                *guard = Some(cx.waker().clone());
            }
            if self.state.triggered.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    /// An ephemeral in-process HTTP server for integration tests with RAII graceful shutdown.
    ///
    /// Binds to `127.0.0.1:0` on a dedicated background Tokio runtime and shuts
    /// down cleanly when dropped.
    pub struct TestServer {
        addr: SocketAddr,
        shutdown: Arc<ShutdownState>,
        thread: Option<JoinHandle<()>>,
    }

    impl TestServer {
        /// Starts serving `app` on an ephemeral localhost port and waits until ready.
        ///
        /// # Panics
        ///
        /// Panics if binding the local TCP socket or building the Tokio runtime fails.
        pub fn start(app: App) -> Self {
            let router = app.into_axum();
            let shutdown = Arc::new(ShutdownState::new());
            let shutdown_rx = Arc::clone(&shutdown);
            let (tx, rx) = mpsc::sync_channel::<io::Result<SocketAddr>>(1);

            let handle = thread::spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                };

                rt.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                        Ok(l) => l,
                        Err(err) => {
                            let _ = tx.send(Err(err));
                            return;
                        }
                    };
                    let addr = match listener.local_addr() {
                        Ok(a) => a,
                        Err(err) => {
                            let _ = tx.send(Err(err));
                            return;
                        }
                    };
                    let _ = tx.send(Ok(addr));

                    let _ = axum::serve(listener, router)
                        .with_graceful_shutdown(ShutdownFuture { state: shutdown_rx })
                        .await;
                });
            });

            let addr = rx
                .recv()
                .expect("test server thread exited before binding")
                .expect("failed to bind TestServer");

            Self {
                addr,
                shutdown,
                thread: Some(handle),
            }
        }

        /// Returns the bound local socket address.
        pub const fn addr(&self) -> SocketAddr {
            self.addr
        }

        /// Returns the base URL (for example, `"http://127.0.0.1:54321"`).
        pub fn base_url(&self) -> String {
            format!("http://{}", self.addr)
        }

        /// Returns a [`TestClient`] connected to this server.
        pub fn client(&self) -> TestClient {
            TestClient::connect(self.addr)
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.shutdown.trigger();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    /// Synchronous HTTP/1.1 test client backed by [`std::net::TcpStream`].
    pub struct TestClient {
        addr: SocketAddr,
        _server: Option<TestServer>,
    }

    impl TestClient {
        /// Starts an owned [`TestServer`] for `app` and returns a client bound to it.
        pub fn new(app: App) -> Self {
            let server = TestServer::start(app);
            let addr = server.addr();
            Self {
                addr,
                _server: Some(server),
            }
        }

        /// Connects to an already-running server at `addr`.
        pub const fn connect(addr: SocketAddr) -> Self {
            Self {
                addr,
                _server: None,
            }
        }

        /// Builds a `GET` request to `path`.
        pub fn get(&self, path: &str) -> TestRequest<'_> {
            self.request("GET", path)
        }

        /// Builds a `POST` request to `path`.
        pub fn post(&self, path: &str) -> TestRequest<'_> {
            self.request("POST", path)
        }

        /// Builds a `PUT` request to `path`.
        pub fn put(&self, path: &str) -> TestRequest<'_> {
            self.request("PUT", path)
        }

        /// Builds a `PATCH` request to `path`.
        pub fn patch(&self, path: &str) -> TestRequest<'_> {
            self.request("PATCH", path)
        }

        /// Builds a `DELETE` request to `path`.
        pub fn delete(&self, path: &str) -> TestRequest<'_> {
            self.request("DELETE", path)
        }

        /// Builds an HTTP request with custom `method` and `path`.
        pub fn request(&self, method: &str, path: &str) -> TestRequest<'_> {
            TestRequest {
                client: self,
                method: method.to_string(),
                path: path.to_string(),
                headers: Vec::new(),
                body: Vec::new(),
            }
        }
    }

    /// Builder for an HTTP/1.1 request issued through [`TestClient`].
    pub struct TestRequest<'a> {
        client: &'a TestClient,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl TestRequest<'_> {
        /// Adds a request header.
        pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
            self.headers.push((name.into(), value.into()));
            self
        }

        /// Sets raw request body bytes.
        pub fn body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
            self.body = bytes.into();
            self
        }

        /// Sets a UTF-8 plain-text body and defaults `Content-Type` to `text/plain; charset=utf-8`.
        pub fn text(mut self, text: impl Into<String>) -> Self {
            if !self
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                self.headers.push((
                    "Content-Type".to_string(),
                    "text/plain; charset=utf-8".to_string(),
                ));
            }
            self.body = text.into().into_bytes();
            self
        }

        /// Sets a JSON body and defaults `Content-Type` to `application/json`.
        pub fn json(mut self, json: impl Into<String>) -> Self {
            if !self
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                self.headers
                    .push(("Content-Type".to_string(), "application/json".to_string()));
            }
            self.body = json.into().into_bytes();
            self
        }

        /// Sends the HTTP/1.1 request and returns the parsed [`TestResponse`].
        pub fn send(self) -> io::Result<TestResponse> {
            let mut stream = TcpStream::connect(self.client.addr)?;

            let path = if self.path.starts_with('/') {
                self.path
            } else {
                format!("/{}", self.path)
            };

            let mut req = format!("{} {} HTTP/1.1\r\n", self.method, path);
            if !self
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("host"))
            {
                req.push_str(&format!("Host: {}\r\n", self.client.addr));
            }
            if !self
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("connection"))
            {
                req.push_str("Connection: close\r\n");
            }
            if !self.body.is_empty()
                && !self
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            {
                req.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
            }
            for (k, v) in &self.headers {
                req.push_str(&format!("{k}: {v}\r\n"));
            }
            req.push_str("\r\n");

            stream.write_all(req.as_bytes())?;
            if !self.body.is_empty() {
                stream.write_all(&self.body)?;
            }
            stream.flush()?;

            parse_http_response(stream)
        }
    }

    fn parse_http_response(stream: TcpStream) -> io::Result<TestResponse> {
        let mut reader = BufReader::new(stream);

        let mut status_line = String::new();
        if reader.read_line(&mut status_line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "empty HTTP response",
            ));
        }
        let trimmed = status_line.trim_end_matches(['\r', '\n']);
        let mut parts = trimmed.splitn(3, ' ');
        let _version = parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP version"))?;
        let status: u16 = parts
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing status code"))?
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid status code"))?;
        let reason = parts.next().unwrap_or("").to_string();

        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }

        let is_chunked = headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("transfer-encoding")
                && v.to_ascii_lowercase().contains("chunked")
        });

        let content_length = headers.iter().find_map(|(k, v)| {
            if k.eq_ignore_ascii_case("content-length") {
                v.parse::<usize>().ok()
            } else {
                None
            }
        });

        let body = if is_chunked {
            read_chunked_body(&mut reader)?
        } else if let Some(len) = content_length {
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            buf
        } else {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf)?;
            buf
        };

        Ok(TestResponse {
            status,
            reason,
            headers,
            body,
        })
    }

    fn read_chunked_body<R: BufRead>(reader: &mut R) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let mut size_line = String::new();
            if reader.read_line(&mut size_line)? == 0 {
                break;
            }
            let size_hex = size_line.trim().split(';').next().unwrap_or("").trim();
            if size_hex.is_empty() {
                continue;
            }
            let size = usize::from_str_radix(size_hex, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))?;
            if size == 0 {
                // Consume trailing headers until empty line.
                let mut trailer = String::new();
                while reader.read_line(&mut trailer)? > 0 {
                    if trailer == "\r\n" || trailer == "\n" {
                        break;
                    }
                    trailer.clear();
                }
                break;
            }
            let start = out.len();
            out.resize(start + size, 0);
            reader.read_exact(&mut out[start..])?;
            // Consume chunk trailing CRLF.
            let mut crlf = [0u8; 2];
            let _ = reader.read_exact(&mut crlf);
        }
        Ok(out)
    }

    /// Parsed HTTP/1.1 response returned by [`TestRequest::send`].
    #[derive(Debug, Clone)]
    pub struct TestResponse {
        status: u16,
        reason: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl TestResponse {
        /// Returns the numeric HTTP status code.
        pub const fn status(&self) -> u16 {
            self.status
        }

        /// Returns the HTTP status reason phrase.
        pub fn reason(&self) -> &str {
            &self.reason
        }

        /// Returns all raw response headers in received order.
        pub fn headers(&self) -> &[(String, String)] {
            &self.headers
        }

        /// Returns the first header value matching `name` (case-insensitive).
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }

        /// Returns all header values matching `name` (case-insensitive).
        pub fn headers_all(&self, name: &str) -> Vec<&str> {
            self.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
                .collect()
        }

        /// Returns the raw response body bytes.
        pub fn bytes(&self) -> &[u8] {
            &self.body
        }

        /// Decodes the response body as UTF-8.
        pub fn text(&self) -> Result<&str, std::str::Utf8Error> {
            std::str::from_utf8(&self.body)
        }

        /// Asserts that the response status equals `expected`.
        #[track_caller]
        pub fn assert_status(&self, expected: u16) -> &Self {
            assert_eq!(
                self.status, expected,
                "expected HTTP status {expected}, got {} ({})",
                self.status, self.reason
            );
            self
        }

        /// Asserts that the response status is a `2xx` success code.
        #[track_caller]
        pub fn assert_ok(&self) -> &Self {
            assert!(
                (200..=299).contains(&self.status),
                "expected 2xx HTTP status, got {} ({})",
                self.status,
                self.reason
            );
            self
        }

        /// Asserts that header `name` exists and equals `expected`.
        #[track_caller]
        pub fn assert_header(&self, name: &str, expected: &str) -> &Self {
            let actual = self.header(name);
            assert_eq!(
                actual,
                Some(expected),
                "expected header {name:?} = {expected:?}, got {actual:?}"
            );
            self
        }

        /// Asserts that the UTF-8 response body contains `needle`.
        #[track_caller]
        pub fn assert_body_contains(&self, needle: &str) -> &Self {
            let body = self.text().expect("response body is not valid UTF-8");
            assert!(
                body.contains(needle),
                "expected response body to contain {needle:?}, got {body:?}"
            );
            self
        }

        /// Asserts that the UTF-8 response body equals `expected`.
        #[track_caller]
        pub fn assert_body_eq(&self, expected: &str) -> &Self {
            let body = self.text().expect("response body is not valid UTF-8");
            assert_eq!(body, expected, "response body mismatch");
            self
        }
    }
}

#[cfg(feature = "http")]
pub use http_test::{TestClient, TestRequest, TestResponse, TestServer};
