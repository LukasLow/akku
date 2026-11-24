//! Zero-dependency blocking Redis client over [`std::net::TcpStream`] implementing
//! the RESP2 wire protocol with depth and bulk-size DoS caps.
//!
//! # Protocol Version
//! The client speaks **RESP2 only**. For interoperability the decoder also accepts the
//! RESP3 null (`_`), boolean (`#`, decoded as `Integer(0/1)`), map (`%`, flattened into an
//! `Array`) and set (`~`) frames, but rejects the remaining RESP3 types (double `,`, big
//! number `(`, blob error `!`, verbatim string `=`, push `>`, attribute `|`) with
//! [`KvError::Protocol`]. Switching the connection to RESP3 via `HELLO 3` is refused by
//! [`RedisClient::command`].

use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use super::{BatchOp, KvError, KvLimits, KvResult, KvStore};

/// Default maximum nesting depth for RESP arrays/maps (64).
pub const DEFAULT_MAX_DEPTH: usize = 64;

/// Default maximum single bulk string size in bytes (16 MiB).
pub const DEFAULT_MAX_BULK_SIZE: usize = 16 * 1024 * 1024;

/// Default maximum number of elements in a single RESP array/collection (65,536).
pub const DEFAULT_MAX_ARRAY_LEN: usize = 65_536;

/// Default maximum single inline header line length in bytes (4 KiB).
pub const DEFAULT_MAX_LINE_LEN: usize = 4 * 1024;

/// Default maximum total bulk payload bytes across one decoded reply (64 MiB).
pub const DEFAULT_MAX_REPLY_BYTES: usize = 64 * 1024 * 1024;

/// Initial allocation chunk for bulk strings; larger payloads grow only as bytes arrive.
const BULK_READ_CHUNK: usize = 64 * 1024;

/// DoS protection limits enforced by the RESP decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RespLimits {
    /// Maximum nesting depth of Aggregate types (`*`, `%`, `~`).
    pub max_depth: usize,
    /// Maximum byte length of a Bulk String (`$`).
    pub max_bulk_size: usize,
    /// Maximum element count in an Array (`*`), Map (`%`), or Set (`~`), and also the
    /// maximum total number of aggregate elements across all nesting levels of one reply.
    pub max_array_len: usize,
    /// Maximum byte length of a `\r\n`-terminated header or simple line.
    pub max_line_len: usize,
    /// Maximum total bulk-string payload bytes across one decoded reply.
    pub max_reply_bytes: usize,
}

impl Default for RespLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_bulk_size: DEFAULT_MAX_BULK_SIZE,
            max_array_len: DEFAULT_MAX_ARRAY_LEN,
            max_line_len: DEFAULT_MAX_LINE_LEN,
            max_reply_bytes: DEFAULT_MAX_REPLY_BYTES,
        }
    }
}

/// A parsed RESP2/RESP3 protocol frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespValue {
    /// `+OK\r\n` — UTF-8 simple string without CR or LF.
    SimpleString(String),
    /// `-ERR message\r\n` — Server error string.
    Error(String),
    /// `:1000\r\n` — Signed 64-bit integer.
    Integer(i64),
    /// `$6\r\nfoobar\r\n` — Binary-safe byte string.
    BulkString(Vec<u8>),
    /// `*2\r\n...` — Ordered list of RESP values.
    Array(Vec<RespValue>),
    /// `$-1\r\n`, `*-1\r\n`, or RESP3 `_\r\n` — Null value.
    Null,
}

impl RespValue {
    /// Encodes this frame into RESP2 wire bytes and appends them to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) -> KvResult<()> {
        match self {
            Self::SimpleString(s) => {
                if s.contains('\r') || s.contains('\n') {
                    return Err(KvError::Protocol(
                        "SimpleString must not contain CR or LF".to_string(),
                    ));
                }
                out.push(b'+');
                out.extend_from_slice(s.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Self::Error(msg) => {
                if msg.contains('\r') || msg.contains('\n') {
                    return Err(KvError::Protocol(
                        "Error message must not contain CR or LF".to_string(),
                    ));
                }
                out.push(b'-');
                out.extend_from_slice(msg.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Self::Integer(n) => {
                out.push(b':');
                out.extend_from_slice(n.to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Self::BulkString(bytes) => {
                out.push(b'$');
                out.extend_from_slice(bytes.len().to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
                out.extend_from_slice(bytes);
                out.extend_from_slice(b"\r\n");
            }
            Self::Array(items) => {
                out.push(b'*');
                out.extend_from_slice(items.len().to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
                for item in items {
                    item.encode(out)?;
                }
            }
            Self::Null => {
                out.extend_from_slice(b"$-1\r\n");
            }
        }
        Ok(())
    }

    /// Encodes this frame into a fresh `Vec<u8>`.
    pub fn to_bytes(&self) -> KvResult<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(&mut out)?;
        Ok(out)
    }

    /// Decodes a single frame from `reader` using [`RespLimits::default()`].
    pub fn decode<R: BufRead>(reader: &mut R) -> KvResult<Self> {
        Self::decode_with_limits(reader, RespLimits::default())
    }

    /// Decodes a single frame from `reader` enforcing custom `limits`.
    pub fn decode_with_limits<R: BufRead>(reader: &mut R, limits: RespLimits) -> KvResult<Self> {
        let mut budget = ReplyBudget {
            elements: limits.max_array_len,
            bytes: limits.max_reply_bytes,
        };
        Self::decode_depth(reader, limits, 0, &mut budget)
    }

    fn decode_depth<R: BufRead>(
        reader: &mut R,
        limits: RespLimits,
        depth: usize,
        budget: &mut ReplyBudget,
    ) -> KvResult<Self> {
        if depth > limits.max_depth {
            return Err(KvError::Protocol(format!(
                "RESP nesting depth {depth} exceeds max_depth {}",
                limits.max_depth
            )));
        }

        let line = read_crlf_line(reader, limits.max_line_len)?;
        if line.is_empty() {
            return Err(KvError::Protocol("empty RESP frame line".to_string()));
        }

        let prefix = line[0];
        let payload = &line[1..];

        match prefix {
            b'+' => {
                budget.take_bytes(payload.len(), limits)?;
                let s = std::str::from_utf8(payload).map_err(|e| {
                    KvError::Protocol(format!("invalid UTF-8 in SimpleString: {e}"))
                })?;
                Ok(Self::SimpleString(s.to_string()))
            }
            b'-' => {
                budget.take_bytes(payload.len(), limits)?;
                let s = std::str::from_utf8(payload)
                    .map_err(|e| KvError::Protocol(format!("invalid UTF-8 in Error frame: {e}")))?;
                Ok(Self::Error(s.to_string()))
            }
            b':' => {
                let n = parse_i64(payload)?;
                Ok(Self::Integer(n))
            }
            b'_' => {
                // RESP3 Null: `_\r\n`
                if !payload.is_empty() {
                    return Err(KvError::Protocol(
                        "unexpected trailing bytes in RESP3 Null frame".to_string(),
                    ));
                }
                Ok(Self::Null)
            }
            b'#' => {
                // RESP3 Boolean: `#t\r\n` or `#f\r\n`
                match payload {
                    b"t" => Ok(Self::Integer(1)),
                    b"f" => Ok(Self::Integer(0)),
                    _ => Err(KvError::Protocol("invalid RESP3 boolean frame".to_string())),
                }
            }
            b'$' => {
                let len = parse_i64(payload)?;
                if len == -1 {
                    return Ok(Self::Null);
                }
                if len < -1 {
                    return Err(KvError::Protocol(format!(
                        "invalid negative bulk string length {len}"
                    )));
                }
                let len = len as usize;
                if len > limits.max_bulk_size {
                    return Err(KvError::Protocol(format!(
                        "RESP bulk string length {len} exceeds max_bulk_size {}",
                        limits.max_bulk_size
                    )));
                }
                budget.take_bytes(len, limits)?;
                // Grow progressively so a lying length prefix cannot force a large allocation
                // before the bytes actually arrive.
                let mut buf = Vec::with_capacity(len.min(BULK_READ_CHUNK));
                reader.by_ref().take(len as u64).read_to_end(&mut buf)?;
                if buf.len() != len {
                    return Err(KvError::Protocol(
                        "unexpected EOF inside RESP bulk string payload".to_string(),
                    ));
                }
                let mut crlf = [0u8; 2];
                reader.read_exact(&mut crlf)?;
                if &crlf != b"\r\n" {
                    return Err(KvError::Protocol(
                        "missing CRLF terminator after bulk string payload".to_string(),
                    ));
                }
                Ok(Self::BulkString(buf))
            }
            b'*' | b'~' => {
                let count = parse_i64(payload)?;
                if count == -1 {
                    return Ok(Self::Null);
                }
                if count < -1 {
                    return Err(KvError::Protocol(format!(
                        "invalid negative array length {count}"
                    )));
                }
                let count = count as usize;
                if count > limits.max_array_len {
                    return Err(KvError::Protocol(format!(
                        "RESP array length {count} exceeds max_array_len {}",
                        limits.max_array_len
                    )));
                }
                budget.take_elements(count, limits)?;
                let mut items = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    items.push(Self::decode_depth(reader, limits, depth + 1, budget)?);
                }
                Ok(Self::Array(items))
            }
            b'%' => {
                // RESP3 Map: `%count\r\n` followed by `2 * count` key/value elements.
                let pairs = parse_i64(payload)?;
                if pairs < 0 {
                    return Err(KvError::Protocol(format!(
                        "invalid negative RESP3 map length {pairs}"
                    )));
                }
                let pairs = pairs as usize;
                let total = pairs.checked_mul(2).ok_or_else(|| {
                    KvError::Protocol("RESP3 map element count overflow".to_string())
                })?;
                if total > limits.max_array_len {
                    return Err(KvError::Protocol(format!(
                        "RESP3 map flattened length {total} exceeds max_array_len {}",
                        limits.max_array_len
                    )));
                }
                budget.take_elements(total, limits)?;
                let mut items = Vec::with_capacity(total.min(1024));
                for _ in 0..total {
                    items.push(Self::decode_depth(reader, limits, depth + 1, budget)?);
                }
                Ok(Self::Array(items))
            }
            b',' | b'(' | b'!' | b'=' | b'>' | b'|' => Err(KvError::Protocol(format!(
                "unsupported RESP3 frame type '{}' (this client speaks RESP2 only)",
                char::from(prefix)
            ))),
            _ => Err(KvError::Protocol(format!(
                "unknown RESP type prefix byte {:#04x}",
                prefix
            ))),
        }
    }

    /// Encodes a command of byte slices as a RESP Array of BulkStrings (`*N\r\n$L\r\n...`).
    pub fn encode_command<I, B>(args: I, out: &mut Vec<u8>) -> KvResult<()>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let collected: Vec<B> = args.into_iter().collect();
        out.push(b'*');
        out.extend_from_slice(collected.len().to_string().as_bytes());
        out.extend_from_slice(b"\r\n");
        for arg in collected {
            let slice = arg.as_ref();
            out.push(b'$');
            out.extend_from_slice(slice.len().to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(slice);
            out.extend_from_slice(b"\r\n");
        }
        Ok(())
    }

    /// Extracts bytes if `BulkString` or `SimpleString`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::BulkString(b) => Some(b),
            Self::SimpleString(s) => Some(s.as_bytes()),
            _ => None,
        }
    }

    /// Extracts UTF-8 `&str` if `SimpleString` or valid UTF-8 `BulkString`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::SimpleString(s) => Some(s),
            Self::BulkString(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    /// Extracts `i64` if `Integer`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(n) => Some(*n),
            _ => None,
        }
    }

    /// Returns `true` if `RespValue::Null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Converts a `RespValue::Error(msg)` into `Err(KvError::Server(msg))`, returning `Ok(self)` otherwise.
    pub fn into_result(self) -> KvResult<Self> {
        match self {
            Self::Error(msg) => Err(KvError::Server(msg)),
            other => Ok(other),
        }
    }
}

/// Remaining per-reply allocation budget shared across all nesting levels.
struct ReplyBudget {
    elements: usize,
    bytes: usize,
}

impl ReplyBudget {
    fn take_bytes(&mut self, count: usize, limits: RespLimits) -> KvResult<()> {
        self.bytes = self.bytes.checked_sub(count).ok_or_else(|| {
            KvError::Protocol(format!(
                "RESP reply payload exceeds max_reply_bytes {}",
                limits.max_reply_bytes
            ))
        })?;
        Ok(())
    }

    fn take_elements(&mut self, count: usize, limits: RespLimits) -> KvResult<()> {
        self.elements = self.elements.checked_sub(count).ok_or_else(|| {
            KvError::Protocol(format!(
                "RESP reply total element count exceeds max_array_len {}",
                limits.max_array_len
            ))
        })?;
        Ok(())
    }
}

fn parse_i64(bytes: &[u8]) -> KvResult<i64> {
    let s = std::str::from_utf8(bytes)
        .map_err(|e| KvError::Protocol(format!("invalid UTF-8 in RESP integer: {e}")))?;
    s.parse::<i64>()
        .map_err(|e| KvError::Protocol(format!("invalid RESP integer '{s}': {e}")))
}

/// Reads a `\r\n`-terminated line from `reader`, enforcing `max_len` (excluding `\r\n`)
/// without allocating unbounded memory if a peer sends an endless line without `\n`.
fn read_crlf_line<R: BufRead>(reader: &mut R, max_len: usize) -> KvResult<Vec<u8>> {
    let mut line = Vec::new();
    let max_with_crlf = max_len.saturating_add(2);

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(KvError::Protocol(
                "unexpected EOF while reading RESP frame line".to_string(),
            ));
        }

        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            if line.len() + pos + 1 > max_with_crlf {
                return Err(KvError::Protocol(format!(
                    "RESP header line exceeds max_line_len {max_len}"
                )));
            }
            line.extend_from_slice(&available[..=pos]);
            reader.consume(pos + 1);
            break;
        } else {
            let chunk_len = available.len();
            if line.len() + chunk_len > max_with_crlf {
                return Err(KvError::Protocol(format!(
                    "RESP header line exceeds max_line_len {max_len}"
                )));
            }
            line.extend_from_slice(available);
            reader.consume(chunk_len);
        }
    }

    if line.len() < 2 || &line[line.len() - 2..] != b"\r\n" {
        return Err(KvError::Protocol(
            "RESP line not terminated with CRLF".to_string(),
        ));
    }
    line.truncate(line.len() - 2);
    Ok(line)
}

/// Live socket halves of one Redis connection.
struct ConnIo {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

/// Connection options for [`RedisClient::connect_with`].
///
/// Authentication and the selected database are replayed on every (re)connection, so a
/// connection transparently re-established after a poisoning error keeps the same session.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RedisOptions {
    /// Timeout for establishing the TCP connection (`None` = OS default).
    pub connect_timeout: Option<Duration>,
    /// Read and write timeout on the socket (`None` = block indefinitely).
    pub io_timeout: Option<Duration>,
    /// Logical database selected with `SELECT` after connecting (`0` = no `SELECT`).
    pub db: u64,
    /// ACL username sent with `AUTH` (requires [`RedisOptions::password`]).
    pub username: Option<String>,
    /// Password sent with `AUTH` after connecting (`None` = no `AUTH`).
    pub password: Option<String>,
}

impl fmt::Debug for RedisOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisOptions")
            .field("connect_timeout", &self.connect_timeout)
            .field("io_timeout", &self.io_timeout)
            .field("db", &self.db)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl RedisOptions {
    /// Creates options with no timeouts, database `0`, and no authentication.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets both the connect and the read/write timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self.io_timeout = Some(timeout);
        self
    }

    /// Selects logical database `db` on every connection.
    pub fn db(mut self, db: u64) -> Self {
        self.db = db;
        self
    }

    /// Authenticates with `AUTH password` on every connection.
    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.username = None;
        self.password = Some(password.into());
        self
    }

    /// Authenticates with the ACL form `AUTH username password` on every connection.
    pub fn auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }
}

/// Session state replayed on every new connection.
#[derive(Clone, Default)]
struct Session {
    /// `(username, password)` for `AUTH`.
    auth: Option<(Option<Vec<u8>>, Vec<u8>)>,
    db: u64,
}

/// How a raw [`RedisClient::command`] affects connection-level session state.
enum CommandKind {
    /// No session state involved.
    Plain,
    /// `SELECT n`: tracked and replayed on reconnect.
    Select(u64),
    /// `AUTH [user] pass`: tracked and replayed on reconnect.
    Auth(Option<Vec<u8>>, Vec<u8>),
    /// `MULTI`.
    Multi,
    /// `WATCH`.
    Watch,
    /// `EXEC` / `DISCARD`: ends MULTI and WATCH.
    EndTxn,
    /// `UNWATCH`.
    Unwatch,
    /// `RESET`: back to a fresh, unauthenticated session on database 0.
    Reset,
    /// State that cannot be replayed (e.g. `CLIENT SETNAME`): pins the connection.
    Pin(&'static str),
}

fn upper(arg: &[u8]) -> Vec<u8> {
    arg.to_ascii_uppercase()
}

/// Classifies a raw command, rejecting commands whose replies do not follow the
/// one-request/one-reply model (pub/sub, MONITOR, client-side tracking, RESP3).
fn classify_command(args: &[&[u8]]) -> KvResult<CommandKind> {
    let Some(name) = args.first() else {
        return Err(KvError::InvalidInput("empty Redis command".to_string()));
    };
    let name = upper(name);
    let unsupported = |what: &str| {
        Err(KvError::InvalidInput(format!(
            "{what} is not supported by RedisClient: it breaks the one-request/one-reply model"
        )))
    };
    let kind = match name.as_slice() {
        b"SUBSCRIBE" | b"PSUBSCRIBE" | b"SSUBSCRIBE" | b"UNSUBSCRIBE" | b"PUNSUBSCRIBE"
        | b"SUNSUBSCRIBE" => return unsupported("pub/sub"),
        b"MONITOR" => return unsupported("MONITOR"),
        b"SYNC" | b"PSYNC" => return unsupported("replication streaming"),
        b"HELLO" => match args.get(1) {
            Some(ver) if *ver != b"2".as_slice() => {
                return Err(KvError::InvalidInput(
                    "HELLO with a protocol version other than 2 is not supported: \
                     RedisClient speaks RESP2 only"
                        .to_string(),
                ))
            }
            Some(_) if args.len() > 2 => CommandKind::Pin("HELLO with AUTH/SETNAME"),
            _ => CommandKind::Plain,
        },
        b"CLIENT" => match args.get(1).map(|a| upper(a)).as_deref() {
            Some(b"TRACKING") => return unsupported("CLIENT TRACKING"),
            Some(b"REPLY") => return unsupported("CLIENT REPLY"),
            Some(b"SETNAME") => CommandKind::Pin("CLIENT SETNAME"),
            Some(b"SETINFO") => CommandKind::Pin("CLIENT SETINFO"),
            Some(b"NO-EVICT") => CommandKind::Pin("CLIENT NO-EVICT"),
            Some(b"NO-TOUCH") => CommandKind::Pin("CLIENT NO-TOUCH"),
            _ => CommandKind::Plain,
        },
        b"READONLY" | b"READWRITE" => CommandKind::Pin("READONLY/READWRITE"),
        b"SELECT" if args.len() == 2 => std::str::from_utf8(args[1])
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map_or(CommandKind::Plain, CommandKind::Select),
        b"AUTH" if args.len() == 2 => CommandKind::Auth(None, args[1].to_vec()),
        b"AUTH" if args.len() == 3 => CommandKind::Auth(Some(args[1].to_vec()), args[2].to_vec()),
        b"MULTI" => CommandKind::Multi,
        b"WATCH" => CommandKind::Watch,
        b"EXEC" | b"DISCARD" => CommandKind::EndTxn,
        b"UNWATCH" => CommandKind::Unwatch,
        b"RESET" => CommandKind::Reset,
        _ => CommandKind::Plain,
    };
    Ok(kind)
}

/// Connection slot. `io` is `None` when the connection was poisoned (an I/O error, timeout,
/// or protocol error happened mid-exchange) and must be re-established before reuse.
struct RedisConn {
    io: Option<ConnIo>,
    addrs: Vec<SocketAddr>,
    connect_timeout: Option<Duration>,
    io_timeout: Option<Duration>,
    session: Session,
    /// Inside `MULTI` (commands are being queued).
    multi: bool,
    /// Keys are being `WATCH`ed.
    watch: bool,
    /// Reason the session holds state that cannot be replayed on a new connection.
    pinned: Option<&'static str>,
}

impl RedisConn {
    fn open(
        addrs: &[SocketAddr],
        connect_timeout: Option<Duration>,
        io_timeout: Option<Duration>,
        session: &Session,
    ) -> KvResult<ConnIo> {
        let mut last_err = None;
        for addr in addrs {
            let res = match connect_timeout {
                Some(t) => TcpStream::connect_timeout(addr, t),
                None => TcpStream::connect(addr),
            };
            match res {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    stream.set_read_timeout(io_timeout)?;
                    stream.set_write_timeout(io_timeout)?;
                    let writer = stream.try_clone()?;
                    let mut io = ConnIo {
                        reader: BufReader::new(stream),
                        writer,
                    };
                    Self::handshake(&mut io, session)?;
                    return Ok(io);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(match last_err {
            Some(e) => KvError::Io(e),
            None => KvError::InvalidInput("could not resolve any socket address".to_string()),
        })
    }

    /// Replays `AUTH` and `SELECT` on a fresh connection.
    fn handshake(io: &mut ConnIo, session: &Session) -> KvResult<()> {
        let mut steps: Vec<Vec<Vec<u8>>> = Vec::new();
        if let Some((user, pass)) = &session.auth {
            let mut cmd = vec![b"AUTH".to_vec()];
            if let Some(user) = user {
                cmd.push(user.clone());
            }
            cmd.push(pass.clone());
            steps.push(cmd);
        }
        if session.db != 0 {
            steps.push(vec![
                b"SELECT".to_vec(),
                session.db.to_string().into_bytes(),
            ]);
        }
        for cmd in steps {
            let mut buf = Vec::new();
            RespValue::encode_command(&cmd, &mut buf)?;
            io.writer.write_all(&buf)?;
            io.writer.flush()?;
            let reply = RespValue::decode(&mut io.reader)?;
            if let RespValue::Error(msg) = reply {
                let what = String::from_utf8_lossy(&cmd[0]).into_owned();
                return Err(KvError::Server(format!(
                    "{what} during connect failed: {msg}"
                )));
            }
        }
        Ok(())
    }

    /// Why a lost connection may not be re-established transparently, if it may not.
    fn reconnect_blocker(&self) -> Option<&'static str> {
        if let Some(reason) = self.pinned {
            return Some(reason);
        }
        if self.multi {
            return Some("MULTI transaction in progress");
        }
        if self.watch {
            return Some("WATCH in progress");
        }
        None
    }

    /// Returns the live connection, reconnecting first if it was poisoned.
    fn ensure(&mut self) -> KvResult<&mut ConnIo> {
        if self.io.is_none() {
            if let Some(reason) = self.reconnect_blocker() {
                return Err(KvError::Protocol(format!(
                    "connection lost while session state could not be restored ({reason}); \
                     call RedisClient::reconnect to start a fresh session"
                )));
            }
            self.io = Some(Self::open(
                &self.addrs,
                self.connect_timeout,
                self.io_timeout,
                &self.session,
            )?);
        }
        match self.io.as_mut() {
            Some(io) => Ok(io),
            None => Err(KvError::Protocol("connection unavailable".to_string())),
        }
    }

    /// Records the session effect of a command whose reply was fully read.
    fn apply(&mut self, kind: CommandKind, ok: bool) {
        if self.multi && ok {
            // Inside MULTI the command is only queued; its effect (if any) depends on EXEC.
            match kind {
                CommandKind::Select(_) | CommandKind::Auth(..) | CommandKind::Pin(_) => {
                    self.pinned = Some("session command queued inside MULTI");
                    return;
                }
                CommandKind::Watch | CommandKind::Unwatch => return,
                _ => {}
            }
        }
        match kind {
            CommandKind::Plain => {}
            CommandKind::Select(db) if ok => self.session.db = db,
            CommandKind::Auth(user, pass) if ok => self.session.auth = Some((user, pass)),
            CommandKind::Multi if ok => self.multi = true,
            CommandKind::Watch if ok => self.watch = true,
            CommandKind::EndTxn if self.multi || ok => {
                self.multi = false;
                self.watch = false;
            }
            CommandKind::Unwatch if ok => self.watch = false,
            CommandKind::Reset if ok => {
                self.session = Session::default();
                self.multi = false;
                self.watch = false;
                self.pinned = None;
            }
            CommandKind::Pin(reason) if ok => self.pinned = Some(reason),
            _ => {}
        }
    }

    /// Discards the connection so no unread reply bytes can ever be handed to a later command.
    fn poison(&mut self) {
        if let Some(io) = self.io.take() {
            let _ = io.writer.shutdown(Shutdown::Both);
        }
    }

    /// Writes `request` and decodes exactly `replies` frames. Any error, and any unsolicited
    /// bytes left buffered after the expected replies, poisons the connection.
    fn exchange(
        &mut self,
        request: &[u8],
        replies: usize,
        limits: RespLimits,
    ) -> KvResult<Vec<RespValue>> {
        let result = (|| {
            let io = self.ensure()?;
            io.writer.write_all(request)?;
            io.writer.flush()?;
            let mut out = Vec::with_capacity(replies.min(1024));
            for _ in 0..replies {
                out.push(RespValue::decode_with_limits(&mut io.reader, limits)?);
            }
            Ok((out, !io.reader.buffer().is_empty()))
        })();
        match result {
            Ok((out, leftover)) => {
                if leftover {
                    self.poison();
                }
                Ok(out)
            }
            Err(e) => {
                self.poison();
                Err(e)
            }
        }
    }
}

/// A zero-dependency blocking Redis client over [`std::net::TcpStream`].
///
/// # Connection Safety
/// If a command fails with an I/O error, timeout, or protocol error while its reply is only
/// partially read, the connection is closed (poisoned) and transparently re-established on the
/// next command. A connection with unread reply bytes is never reused, so a late or garbled
/// reply can never be returned as the answer to a different command. Server error replies
/// (`-ERR ...`) are complete frames and keep the connection.
///
/// # Session State
/// `AUTH` and `SELECT` — from [`RedisOptions`] or issued through [`RedisClient::command`] —
/// are tracked and replayed on every reconnection. State that cannot be replayed
/// (`MULTI`/`WATCH` transactions, `CLIENT SETNAME`, `HELLO` with options, `READONLY`, …)
/// disables transparent reconnection: once such a connection is lost, commands fail with
/// [`KvError::Protocol`] instead of silently running on a fresh session. `EXEC`/`DISCARD`
/// (or `UNWATCH`) on a lost transaction report the abort and clear the transaction state;
/// [`RedisClient::reconnect`] or `RESET` starts a fresh session. Pub/sub, `MONITOR`,
/// `CLIENT TRACKING`, `CLIENT REPLY` and `HELLO 3` are rejected with
/// [`KvError::InvalidInput`] because their replies do not follow the request/reply model.
pub struct RedisClient {
    conn: Mutex<RedisConn>,
    resp_limits: RespLimits,
    kv_limits: KvLimits,
}

impl fmt::Debug for RedisClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisClient")
            .field("resp_limits", &self.resp_limits)
            .field("kv_limits", &self.kv_limits)
            .finish()
    }
}

impl RedisClient {
    fn from_parts(addrs: Vec<SocketAddr>, options: RedisOptions) -> KvResult<Self> {
        let session = Session {
            auth: options
                .password
                .map(|pass| (options.username.map(String::into_bytes), pass.into_bytes())),
            db: options.db,
        };
        let io = RedisConn::open(
            &addrs,
            options.connect_timeout,
            options.io_timeout,
            &session,
        )?;
        Ok(Self {
            conn: Mutex::new(RedisConn {
                io: Some(io),
                addrs,
                connect_timeout: options.connect_timeout,
                io_timeout: options.io_timeout,
                session,
                multi: false,
                watch: false,
                pinned: None,
            }),
            resp_limits: RespLimits::default(),
            kv_limits: KvLimits::default(),
        })
    }

    /// Connects to a Redis server at `addr` with default [`RespLimits`] and [`KvLimits`].
    pub fn connect<A: ToSocketAddrs>(addr: A) -> KvResult<Self> {
        Self::from_parts(addr.to_socket_addrs()?.collect(), RedisOptions::default())
    }

    /// Connects to a Redis server at `addr` with explicit [`RedisOptions`] (timeouts,
    /// `AUTH` credentials, and database), which are replayed on every reconnection.
    ///
    /// Fails with [`KvError::Server`] if the server rejects `AUTH` or `SELECT`.
    pub fn connect_with<A: ToSocketAddrs>(addr: A, options: RedisOptions) -> KvResult<Self> {
        Self::from_parts(addr.to_socket_addrs()?.collect(), options)
    }

    /// Connects to a Redis server at `addr` with a connect, read, and write `timeout`.
    pub fn connect_timeout<A: ToSocketAddrs>(addr: A, timeout: Duration) -> KvResult<Self> {
        Self::from_parts(
            addr.to_socket_addrs()?.collect(),
            RedisOptions::new().timeout(timeout),
        )
    }

    fn lock_conn(&self) -> std::sync::MutexGuard<'_, RedisConn> {
        match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // A panic mid-exchange may have left unread reply bytes: drop the connection.
                let mut guard = poisoned.into_inner();
                guard.poison();
                self.conn.clear_poison();
                guard
            }
        }
    }

    /// Overrides the [`RespLimits`] enforced when decoding server responses.
    pub fn with_limits(mut self, limits: RespLimits) -> Self {
        self.resp_limits = limits;
        self
    }

    /// Overrides the [`KvLimits`] enforced on keys and values.
    pub fn with_kv_limits(mut self, limits: KvLimits) -> Self {
        self.kv_limits = limits;
        self
    }

    /// Sets the read and write timeouts on the underlying TCP socket.
    ///
    /// The timeouts are also applied to connections re-established after a poisoning error.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> KvResult<()> {
        let mut guard = self.lock_conn();
        guard.io_timeout = timeout;
        if let Some(io) = guard.io.as_ref() {
            io.writer.set_read_timeout(timeout)?;
            io.writer.set_write_timeout(timeout)?;
        }
        Ok(())
    }

    /// Drops the current connection and all non-replayable session state (transactions,
    /// pinned state such as `CLIENT SETNAME`), then connects afresh replaying `AUTH` and
    /// `SELECT`.
    pub fn reconnect(&self) -> KvResult<()> {
        let mut guard = self.lock_conn();
        guard.poison();
        guard.multi = false;
        guard.watch = false;
        guard.pinned = None;
        guard.ensure()?;
        Ok(())
    }

    /// Sends an arbitrary Redis command (e.g. `&["SET", "k", "v"]`) and returns the decoded [`RespValue`].
    ///
    /// If the server replies with `RespValue::Error(msg)`, `command` returns `Err(KvError::Server(msg))`.
    /// Session-changing commands are tracked as described in
    /// [Session State](RedisClient#session-state); pub/sub, `MONITOR`, `CLIENT TRACKING`,
    /// `CLIENT REPLY`, and `HELLO 3` are rejected with [`KvError::InvalidInput`].
    pub fn command<I, B>(&self, args: I) -> KvResult<RespValue>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let args: Vec<B> = args.into_iter().collect();
        let refs: Vec<&[u8]> = args.iter().map(AsRef::as_ref).collect();
        let kind = classify_command(&refs)?;
        let mut buf = Vec::new();
        RespValue::encode_command(&refs, &mut buf)?;
        let mut guard = self.lock_conn();
        if guard.io.is_none() && guard.pinned.is_none() {
            let ends_lost_txn = match kind {
                CommandKind::EndTxn => guard.multi || guard.watch,
                CommandKind::Unwatch => guard.watch && !guard.multi,
                _ => false,
            };
            if ends_lost_txn {
                guard.multi = false;
                guard.watch = false;
                return Err(KvError::Protocol(
                    "transaction aborted: the connection was lost during MULTI/WATCH".to_string(),
                ));
            }
        }
        let mut replies = guard.exchange(&buf, 1, self.resp_limits)?;
        let resp = match replies.pop() {
            Some(resp) => resp,
            None => return Err(KvError::Protocol("missing RESP reply".to_string())),
        };
        guard.apply(kind, !matches!(resp, RespValue::Error(_)));
        drop(guard);
        resp.into_result()
    }

    /// Sends `PING` and returns the server response string (typically `"PONG"`).
    pub fn ping(&self) -> KvResult<String> {
        let resp = self.command([b"PING".as_slice()])?;
        match resp.as_str() {
            Some(s) => Ok(s.to_string()),
            None => Err(KvError::Protocol(format!(
                "unexpected PING response: {resp:?}"
            ))),
        }
    }

    /// Fetches the raw bytes for `key` (`GET key`).
    pub fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>> {
        self.kv_limits.validate_key(key)?;
        let resp = self.command([b"GET".as_slice(), key.as_bytes()])?;
        match resp {
            RespValue::Null => Ok(None),
            RespValue::BulkString(b) => Ok(Some(b)),
            RespValue::SimpleString(s) => Ok(Some(s.into_bytes())),
            other => Err(KvError::Protocol(format!(
                "unexpected GET response: {other:?}"
            ))),
        }
    }

    /// Sets `key` to `value` (`SET key value`).
    pub fn set(&self, key: &str, value: &[u8]) -> KvResult<()> {
        self.kv_limits.validate_kv(key, value)?;
        let _ = self.command([b"SET".as_slice(), key.as_bytes(), value])?;
        Ok(())
    }

    /// Sets `key` to `value` with an expiration (`SET key value PX <millis>`).
    pub fn set_ex(&self, key: &str, value: &[u8], ttl: Duration) -> KvResult<()> {
        self.kv_limits.validate_kv(key, value)?;
        let millis = ttl.as_millis().max(1).to_string();
        let _ = self.command([
            b"SET".as_slice(),
            key.as_bytes(),
            value,
            b"PX".as_slice(),
            millis.as_bytes(),
        ])?;
        Ok(())
    }

    /// Deletes one or more keys (`DEL k1 k2 ...`) and returns the number of keys removed.
    pub fn del(&self, keys: &[&str]) -> KvResult<u64> {
        if keys.is_empty() {
            return Ok(0);
        }
        for k in keys {
            self.kv_limits.validate_key(k)?;
        }
        let mut args: Vec<&[u8]> = Vec::with_capacity(1 + keys.len());
        args.push(b"DEL");
        for k in keys {
            args.push(k.as_bytes());
        }
        let resp = self.command(args)?;
        match resp {
            RespValue::Integer(n) if n >= 0 => Ok(n as u64),
            other => Err(KvError::Protocol(format!(
                "unexpected DEL response: {other:?}"
            ))),
        }
    }

    /// Returns all keys matching glob `pattern` (`KEYS pattern`), sorted lexicographically.
    pub fn keys(&self, pattern: &str) -> KvResult<Vec<String>> {
        let resp = self.command([b"KEYS".as_slice(), pattern.as_bytes()])?;
        let mut out = extract_string_array(resp)?;
        out.sort();
        Ok(out)
    }

    /// Performs a cursor-based `SCAN <cursor> MATCH <pattern> COUNT <count>` iteration,
    /// returning `(next_cursor, keys)`.
    pub fn scan_page(
        &self,
        cursor: u64,
        pattern: &str,
        count: usize,
    ) -> KvResult<(u64, Vec<String>)> {
        let cursor_str = cursor.to_string();
        let count_str = count.to_string();
        let resp = self.command([
            b"SCAN".as_slice(),
            cursor_str.as_bytes(),
            b"MATCH".as_slice(),
            pattern.as_bytes(),
            b"COUNT".as_slice(),
            count_str.as_bytes(),
        ])?;
        match resp {
            RespValue::Array(mut parts) if parts.len() == 2 => {
                let keys_part = parts.pop().unwrap();
                let cursor_part = parts.pop().unwrap();
                let next_cursor = match cursor_part {
                    RespValue::Integer(n) if n >= 0 => n as u64,
                    other => match other.as_str() {
                        Some(s) => s.parse::<u64>().map_err(|e| {
                            KvError::Protocol(format!("invalid SCAN cursor '{s}': {e}"))
                        })?,
                        None => {
                            return Err(KvError::Protocol(format!(
                                "invalid SCAN cursor frame: {other:?}"
                            )))
                        }
                    },
                };
                let keys = extract_string_array(keys_part)?;
                Ok((next_cursor, keys))
            }
            other => Err(KvError::Protocol(format!(
                "unexpected SCAN response structure: {other:?}"
            ))),
        }
    }

    /// Iterates `SCAN` from cursor `0` until cursor returns to `0`, collecting all matching keys
    /// in deduplicated, lexicographically sorted order.
    pub fn scan(&self, pattern: &str) -> KvResult<Vec<String>> {
        let mut cursor = 0u64;
        let mut all_keys = Vec::new();
        loop {
            let (next, page) = self.scan_page(cursor, pattern, 100)?;
            all_keys.extend(page);
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        all_keys.sort();
        all_keys.dedup();
        Ok(all_keys)
    }
}

fn extract_string_array(resp: RespValue) -> KvResult<Vec<String>> {
    match resp {
        RespValue::Array(items) => {
            let mut res = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(s) => res.push(s.to_string()),
                    None => {
                        return Err(KvError::Protocol(format!(
                            "expected string element in array, got {item:?}"
                        )))
                    }
                }
            }
            Ok(res)
        }
        RespValue::Null => Ok(Vec::new()),
        other => Err(KvError::Protocol(format!(
            "expected RESP Array, got {other:?}"
        ))),
    }
}

fn escape_glob_prefix(prefix: &str) -> String {
    let mut pattern = String::with_capacity(prefix.len() + 2);
    for ch in prefix.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('*');
    pattern
}

impl KvStore for RedisClient {
    fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>> {
        RedisClient::get(self, key)
    }

    fn set(&self, key: &str, value: &[u8]) -> KvResult<()> {
        RedisClient::set(self, key, value)
    }

    fn set_ex(&self, key: &str, value: &[u8], ttl: Duration) -> KvResult<()> {
        RedisClient::set_ex(self, key, value, ttl)
    }

    fn delete(&self, key: &str) -> KvResult<bool> {
        Ok(self.del(&[key])? > 0)
    }

    fn exists(&self, key: &str) -> KvResult<bool> {
        self.kv_limits.validate_key(key)?;
        let resp = self.command([b"EXISTS".as_slice(), key.as_bytes()])?;
        match resp {
            RespValue::Integer(n) => Ok(n > 0),
            other => Err(KvError::Protocol(format!(
                "unexpected EXISTS response: {other:?}"
            ))),
        }
    }

    fn scan_prefix(&self, prefix: &str) -> KvResult<Vec<(String, Vec<u8>)>> {
        let pattern = escape_glob_prefix(prefix);
        let keys = self.scan(&pattern)?;
        let mut results = Vec::with_capacity(keys.len());
        for k in keys {
            if k.starts_with(prefix) {
                if let Some(val) = RedisClient::get(self, &k)? {
                    results.push((k, val));
                }
            }
        }
        Ok(results)
    }

    fn batch(&self, ops: &[BatchOp]) -> KvResult<()> {
        if ops.is_empty() {
            return Ok(());
        }

        let mut pipeline_buf = Vec::new();
        for op in ops {
            match op {
                BatchOp::Set { key, value } => {
                    self.kv_limits.validate_kv(key, value)?;
                    RespValue::encode_command(
                        [b"SET".as_slice(), key.as_bytes(), value.as_slice()],
                        &mut pipeline_buf,
                    )?;
                }
                BatchOp::SetEx { key, value, ttl } => {
                    self.kv_limits.validate_kv(key, value)?;
                    let millis = ttl.as_millis().max(1).to_string();
                    RespValue::encode_command(
                        [
                            b"SET".as_slice(),
                            key.as_bytes(),
                            value.as_slice(),
                            b"PX".as_slice(),
                            millis.as_bytes(),
                        ],
                        &mut pipeline_buf,
                    )?;
                }
                BatchOp::Delete { key } => {
                    self.kv_limits.validate_key(key)?;
                    RespValue::encode_command(
                        [b"DEL".as_slice(), key.as_bytes()],
                        &mut pipeline_buf,
                    )?;
                }
            }
        }

        let mut guard = self.lock_conn();
        let replies = guard.exchange(&pipeline_buf, ops.len(), self.resp_limits)?;
        drop(guard);

        let mut first_err: Option<KvError> = None;
        for resp in replies {
            if let Err(e) = resp.into_result() {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        if let Some(err) = first_err {
            return Err(err);
        }
        Ok(())
    }
}
