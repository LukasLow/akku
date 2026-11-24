//! Integration tests for `akku::ws` (RFC 6455 framing, handshake, state machine, TCP transport).

use std::io::{self, Cursor, Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use akku::ws::client::{self, ClientBuilder};
use akku::ws::handshake::{compute_accept_key, generate_key, is_valid_key, WEBSOCKET_GUID};
use akku::ws::server::{self, AcceptResponse, RejectResponse};
use akku::ws::{
    apply_mask, CloseCode, CloseFrame, Config, Error, Frame, Message, OpCode, Role, WebSocket,
};

/// In-memory duplex stream (reads from `incoming`, writes to `outgoing`) for deterministic tests.
#[derive(Debug)]
struct MockStream {
    incoming: Cursor<Vec<u8>>,
    outgoing: Vec<u8>,
}

impl MockStream {
    fn new(incoming: Vec<u8>) -> Self {
        Self {
            incoming: Cursor::new(incoming),
            outgoing: Vec::new(),
        }
    }
}

impl Read for MockStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.incoming.read(buf)
    }
}

impl Write for MockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.outgoing.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn rfc6455_golden_accept_key_vector() {
    assert_eq!(WEBSOCKET_GUID, "258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    assert_eq!(
        compute_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

#[test]
fn generate_key_and_validation() {
    let k1 = generate_key();
    let k2 = generate_key();
    assert_eq!(k1.len(), 24);
    assert!(is_valid_key(&k1));
    assert!(is_valid_key(&k2));
    assert_ne!(k1, k2);
    assert!(is_valid_key("dGhlIHNhbXBsZSBub25jZQ=="));
    assert!(!is_valid_key(""));
    assert!(!is_valid_key("short"));
    assert!(!is_valid_key("dGhlIHNhbXBsZSBub25jZQ=")); // wrong padding/length
    assert!(!is_valid_key("AAAA!!!!BBBBCCCCDDDDEE==")); // invalid base64
    assert!(is_valid_key("AAAAAAAAAAAAAAAAAAAAAA==")); // 16 zero bytes is valid Base64 length
    assert!(!is_valid_key("AAAAAAAAAAAAAAAAAAAA")); // 15 bytes (20 chars)
    assert!(!is_valid_key("AAAAAAAAAAAAAAAAAAAAAAA=")); // 17 bytes (24 chars)
}

#[test]
fn wire_vector_unmasked_text_hello() {
    // RFC 6455 §5.7: single-frame unmasked text message "Hello"
    let expected = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
    let frame = Frame::text("Hello");
    let wire = frame.encode().unwrap();
    assert_eq!(wire, expected);

    let mut cur = Cursor::new(wire);
    let decoded = Frame::read_from(&mut cur, 1024).unwrap();
    assert!(decoded.fin);
    assert_eq!(decoded.opcode, OpCode::Text);
    assert_eq!(decoded.mask, None);
    assert_eq!(decoded.payload, b"Hello");
}

#[test]
fn wire_vector_masked_text_hello() {
    // RFC 6455 §5.7: single-frame masked text message "Hello"
    let mask = [0x37, 0xfa, 0x21, 0x3d];
    let expected = [
        0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
    ];
    let frame = Frame::text("Hello").with_mask(mask);
    let wire = frame.encode().unwrap();
    assert_eq!(wire, expected);

    let mut cur = Cursor::new(wire);
    let decoded = Frame::read_from(&mut cur, 1024).unwrap();
    assert!(decoded.fin);
    assert_eq!(decoded.opcode, OpCode::Text);
    assert_eq!(decoded.mask, Some(mask));
    assert_eq!(decoded.payload, b"Hello");
}

#[test]
fn apply_mask_various_lengths_and_involution() {
    let mask = [0xDE, 0xAD, 0xBE, 0xEF];
    for len in [0, 1, 3, 4, 7, 8, 9, 15, 16, 17, 31, 64, 1023] {
        let original: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
        let mut buf = original.clone();
        apply_mask(&mut buf, mask);
        for (i, &b) in buf.iter().enumerate() {
            assert_eq!(b, original[i] ^ mask[i & 3]);
        }
        apply_mask(&mut buf, mask);
        assert_eq!(buf, original);
    }
}

#[test]
fn extended_payload_lengths_125_126_65535_65536() {
    let cases = [
        (0usize, 2usize),
        (125, 2),
        (126, 4),
        (65_535, 4),
        (65_536, 10),
    ];

    for (len, expected_hdr_unmasked) in cases {
        let payload: Vec<u8> = (0..len).map(|i| (i & 0xFF) as u8).collect();

        // Unmasked
        let frame = Frame::binary(payload.clone());
        let wire = frame.encode().unwrap();
        assert_eq!(wire.len(), expected_hdr_unmasked + len);
        let decoded = Frame::read_from(&mut Cursor::new(wire), 128 * 1024).unwrap();
        assert_eq!(decoded.payload, payload);

        // Masked (adds 4 bytes to header)
        let mask = [0x11, 0x22, 0x33, 0x44];
        let masked_frame = Frame::binary(payload.clone()).with_mask(mask);
        let masked_wire = masked_frame.encode().unwrap();
        assert_eq!(masked_wire.len(), expected_hdr_unmasked + 4 + len);
        let decoded_masked = Frame::read_from(&mut Cursor::new(masked_wire), 128 * 1024).unwrap();
        assert_eq!(decoded_masked.mask, Some(mask));
        assert_eq!(decoded_masked.payload, payload);
    }
}

#[test]
fn rejects_non_zero_rsv_bits() {
    for rsv in [0x40u8, 0x20, 0x10, 0x70] {
        let raw = [0x80 | rsv | 0x01, 0x00];
        let err = Frame::read_from(&mut Cursor::new(raw), 1024).unwrap_err();
        assert!(matches!(err, Error::Protocol(_)), "unexpected err: {err:?}");
    }
}

#[test]
fn rejects_reserved_opcodes() {
    for op in [0x3u8, 0x4, 0x5, 0x6, 0x7, 0xB, 0xC, 0xD, 0xE, 0xF] {
        let raw = [0x80 | op, 0x00];
        let err = Frame::read_from(&mut Cursor::new(raw), 1024).unwrap_err();
        assert!(matches!(err, Error::InvalidOpcode(x) if x == op));
    }
}

#[test]
fn rejects_non_minimal_length_and_msb_set() {
    // 16-bit length encoding for payload length 100 (< 126) -> protocol violation
    let non_min_16 = [0x82, 126, 0x00, 100];
    assert!(matches!(
        Frame::read_from(&mut Cursor::new(non_min_16), 1024),
        Err(Error::Protocol(_))
    ));

    // 64-bit length encoding for payload length 1000 (<= 65535) -> protocol violation
    let non_min_64 = [0x82, 127, 0, 0, 0, 0, 0, 0, 0x03, 0xE8];
    assert!(matches!(
        Frame::read_from(&mut Cursor::new(non_min_64), 1024),
        Err(Error::Protocol(_))
    ));

    // 64-bit length with MSB=1 -> protocol violation
    let msb_set = [0x82, 127, 0x80, 0, 0, 0, 0, 1, 0, 0];
    assert!(matches!(
        Frame::read_from(&mut Cursor::new(msb_set), usize::MAX),
        Err(Error::Protocol(_))
    ));
}

#[test]
fn rejects_fragmented_or_oversized_control_frames() {
    // Encode rejects fragmented Ping
    let frag_ping = Frame::new(false, OpCode::Ping, b"hi".to_vec());
    assert!(matches!(frag_ping.encode(), Err(Error::Protocol(_))));

    // Encode rejects 126-byte Ping
    let big_ping = Frame::ping(vec![b'a'; 126]);
    assert!(matches!(big_ping.encode(), Err(Error::Protocol(_))));

    // Decode rejects fragmented Ping on wire
    let wire_frag_ping = [0x09, 0x02, b'h', b'i'];
    assert!(matches!(
        Frame::read_from(&mut Cursor::new(wire_frag_ping), 1024),
        Err(Error::Protocol(_))
    ));

    // Decode rejects 126-byte Pong on wire
    let mut wire_big_pong = vec![0x8A, 126, 0x00, 126];
    wire_big_pong.extend_from_slice(&[0u8; 126]);
    assert!(matches!(
        Frame::read_from(&mut Cursor::new(wire_big_pong), 1024),
        Err(Error::Protocol(_))
    ));
}

#[test]
fn dos_cap_rejects_oversized_frame_without_allocating() {
    // Header claims 100 MiB payload, but stream has ZERO payload bytes after header.
    // Must immediately return FrameTooLarge without trying to read or allocate 100 MiB.
    let declared: u64 = 100 * 1024 * 1024;
    let mut header = vec![0x82, 127];
    header.extend_from_slice(&declared.to_be_bytes());

    let err = Frame::read_from(&mut Cursor::new(header), 1024).unwrap_err();
    assert!(matches!(
        err,
        Error::FrameTooLarge { len, max: 1024 } if len == declared
    ));
}

#[test]
fn role_masking_enforcement_server_and_client() {
    // Server must reject unmasked frame from client
    let unmasked = Frame::text("oops").encode().unwrap();
    let mut server_ws = WebSocket::from_raw(MockStream::new(unmasked), Role::Server, Config::new());
    assert!(matches!(server_ws.recv(), Err(Error::Protocol(_))));

    // Client must reject masked frame from server
    let masked = Frame::text("oops")
        .with_mask([1, 2, 3, 4])
        .encode()
        .unwrap();
    let mut client_ws = WebSocket::from_raw(MockStream::new(masked), Role::Client, Config::new());
    assert!(matches!(client_ws.recv(), Err(Error::Protocol(_))));
}

#[test]
fn fragmented_text_splits_multibyte_utf8_across_frames() {
    // "hi 🦀!" where 🦀 is [0xF0, 0x9F, 0xA6, 0x80], split across 3 fragments
    let f1 = Frame::new(false, OpCode::Text, vec![b'h', b'i', b' ', 0xF0, 0x9F])
        .encode()
        .unwrap();
    let f2 = Frame::new(false, OpCode::Continuation, vec![0xA6])
        .encode()
        .unwrap();
    let f3 = Frame::new(true, OpCode::Continuation, vec![0x80, b'!'])
        .encode()
        .unwrap();

    let mut wire = Vec::new();
    wire.extend_from_slice(&f1);
    wire.extend_from_slice(&f2);
    wire.extend_from_slice(&f3);

    let mut ws = WebSocket::from_raw(MockStream::new(wire), Role::Client, Config::new());
    assert_eq!(ws.recv().unwrap(), Message::Text("hi 🦀!".to_string()));
}

#[test]
fn streaming_utf8_fails_fast_on_first_bad_fragment() {
    // First fragment already contains invalid UTF-8 byte 0xFF, and no continuation frame exists.
    // Fail-fast streaming validation must return InvalidUtf8 (not UnexpectedEof).
    let f1 = Frame::new(false, OpCode::Text, vec![b'a', 0xFF, b'b'])
        .encode()
        .unwrap();
    let mut ws = WebSocket::from_raw(MockStream::new(f1), Role::Client, Config::new());
    assert!(matches!(ws.recv(), Err(Error::InvalidUtf8(_))));
}

#[test]
fn fragmented_text_rejects_truncated_multibyte_at_fin() {
    // Final fragment leaves an incomplete 4-byte UTF-8 sequence.
    let f1 = Frame::new(false, OpCode::Text, vec![0xF0, 0x9F])
        .encode()
        .unwrap();
    let f2 = Frame::new(true, OpCode::Continuation, vec![0xA6])
        .encode()
        .unwrap();

    let mut wire = Vec::new();
    wire.extend_from_slice(&f1);
    wire.extend_from_slice(&f2);

    let mut ws = WebSocket::from_raw(MockStream::new(wire), Role::Client, Config::new());
    assert!(matches!(ws.recv(), Err(Error::InvalidUtf8(_))));
}

#[test]
fn interleaved_ping_during_fragmentation_triggers_auto_pong() {
    let f1 = Frame::new(false, OpCode::Text, b"hel".to_vec())
        .encode()
        .unwrap();
    let ping = Frame::ping(b"hb".to_vec()).encode().unwrap();
    let f2 = Frame::new(true, OpCode::Continuation, b"lo".to_vec())
        .encode()
        .unwrap();

    let mut wire = Vec::new();
    wire.extend_from_slice(&f1);
    wire.extend_from_slice(&ping);
    wire.extend_from_slice(&f2);

    let mut ws = WebSocket::from_raw(MockStream::new(wire), Role::Client, Config::new());
    // First recv yields the interleaved Ping
    assert_eq!(ws.recv().unwrap(), Message::Ping(b"hb".to_vec()));
    // Second recv completes the reassembled Text message
    assert_eq!(ws.recv().unwrap(), Message::Text("hello".to_string()));

    // Verify client automatically sent a masked Pong("hb")
    let out = ws.into_inner().outgoing;
    let reply = Frame::read_from(&mut Cursor::new(out), 1024).unwrap();
    assert_eq!(reply.opcode, OpCode::Pong);
    assert!(reply.mask.is_some());
    assert_eq!(reply.payload, b"hb");
}

#[test]
fn auto_pong_can_be_disabled() {
    let ping = Frame::ping(b"quiet".to_vec()).encode().unwrap();
    let cfg = Config::new().with_auto_pong(false);
    let mut ws = WebSocket::from_raw(MockStream::new(ping), Role::Client, cfg);
    assert_eq!(ws.recv().unwrap(), Message::Ping(b"quiet".to_vec()));
    assert!(ws.into_inner().outgoing.is_empty());
}

#[test]
fn rejects_invalid_fragment_transitions() {
    // Unexpected Continuation without starting fragment
    let orphan = Frame::new(true, OpCode::Continuation, b"abc".to_vec())
        .encode()
        .unwrap();
    let mut ws1 = WebSocket::from_raw(MockStream::new(orphan), Role::Client, Config::new());
    assert!(matches!(ws1.recv(), Err(Error::Protocol(_))));

    // New Binary data frame while fragmented Text is still open
    let mut wire = Frame::new(false, OpCode::Text, b"part1".to_vec())
        .encode()
        .unwrap();
    wire.extend_from_slice(&Frame::binary(b"intruder".to_vec()).encode().unwrap());
    let mut ws2 = WebSocket::from_raw(MockStream::new(wire), Role::Client, Config::new());
    assert!(matches!(ws2.recv(), Err(Error::Protocol(_))));
}

#[test]
fn dos_cap_rejects_oversized_reassembled_message() {
    let f1 = Frame::new(false, OpCode::Binary, vec![1u8; 500])
        .encode()
        .unwrap();
    let f2 = Frame::new(false, OpCode::Continuation, vec![2u8; 500])
        .encode()
        .unwrap();
    let f3 = Frame::new(true, OpCode::Continuation, vec![3u8; 500])
        .encode()
        .unwrap();

    let mut wire = Vec::new();
    wire.extend_from_slice(&f1);
    wire.extend_from_slice(&f2);
    wire.extend_from_slice(&f3);

    let cfg = Config::new()
        .with_max_frame_size(1024)
        .with_max_message_size(1024);
    let mut ws = WebSocket::from_raw(MockStream::new(wire), Role::Client, cfg);
    assert!(matches!(
        ws.recv(),
        Err(Error::MessageTooLarge {
            len: 1500,
            max: 1024
        })
    ));
}

#[test]
fn close_code_and_close_frame_validation() {
    assert!(CloseCode::NORMAL.is_allowed_on_wire());
    assert!(CloseCode::new(3000).is_allowed_on_wire());
    assert!(CloseCode::new(4999).is_allowed_on_wire());
    for forbidden in [0, 999, 1004, 1005, 1006, 1015, 1016, 2999, 5000] {
        assert!(
            !CloseCode::new(forbidden).is_allowed_on_wire(),
            "code {forbidden} should be forbidden"
        );
        let cf = CloseFrame::new(CloseCode::new(forbidden), "");
        assert!(matches!(
            cf.encode_payload(),
            Err(Error::InvalidCloseCode(c)) if c == forbidden
        ));
    }

    // Empty close payload -> Ok(None)
    assert_eq!(CloseFrame::decode_payload(&[]).unwrap(), None);

    // 1-byte close payload -> Protocol error
    assert!(matches!(
        CloseFrame::decode_payload(&[0x03]),
        Err(Error::Protocol(_))
    ));

    // Invalid UTF-8 close reason -> InvalidUtf8
    let mut bad_utf8 = 1000u16.to_be_bytes().to_vec();
    bad_utf8.extend_from_slice(&[0xFF, 0xFE]);
    assert!(matches!(
        CloseFrame::decode_payload(&bad_utf8),
        Err(Error::InvalidUtf8(_))
    ));

    // Valid close roundtrip + auto-close reply
    let cf = CloseFrame::new(CloseCode::GOING_AWAY, "bye");
    let wire = Frame::close(Some(&cf)).unwrap().encode().unwrap();
    let mut ws = WebSocket::from_raw(MockStream::new(wire), Role::Client, Config::new());
    assert_eq!(ws.recv().unwrap(), Message::Close(Some(cf)));
    // Subsequent send after close must return ConnectionClosed
    assert!(matches!(
        ws.send_text("too late"),
        Err(Error::ConnectionClosed)
    ));
    // Verify auto-close reply was written
    let reply = Frame::read_from(&mut Cursor::new(ws.into_inner().outgoing), 1024).unwrap();
    assert_eq!(reply.opcode, OpCode::Close);
    let reply_cf = CloseFrame::decode_payload(&reply.payload).unwrap().unwrap();
    assert_eq!(reply_cf.code, CloseCode::GOING_AWAY);
}

#[test]
fn handshake_accepts_multi_token_connection_and_case_insensitive_headers() {
    let req = concat!(
        "GET /chat?room=1 HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Upgrade: WebSocket\r\n",
        "Connection: keep-alive, Upgrade\r\n",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Version: 13\r\n",
        "\r\n"
    );
    let stream = MockStream::new(req.as_bytes().to_vec());
    let ws = server::accept(stream).unwrap();
    let resp = String::from_utf8(ws.into_inner().outgoing).unwrap();
    assert!(resp.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));
    assert!(resp.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
}

#[test]
fn handshake_rejects_wrong_version_with_426() {
    let req = concat!(
        "GET / HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Upgrade: websocket\r\n",
        "Connection: Upgrade\r\n",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Version: 12\r\n",
        "\r\n"
    );
    let mut stream = MockStream::new(req.as_bytes().to_vec());
    let err = server::accept(&mut stream).unwrap_err();
    assert!(matches!(err, Error::Handshake(_)));
    let resp = String::from_utf8(stream.outgoing).unwrap();
    assert!(resp.starts_with("HTTP/1.1 426 Upgrade Required\r\n"));
    assert!(resp.contains("Sec-WebSocket-Version: 13\r\n"));
}

#[test]
fn handshake_smuggling_guards_reject_bodies_duplicates_and_bad_keys() {
    // Request smuggling: Transfer-Encoding on Upgrade request
    let smuggle_te = concat!(
        "GET / HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Upgrade: websocket\r\n",
        "Connection: Upgrade\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Version: 13\r\n",
        "\r\n"
    );
    assert!(matches!(
        server::accept(MockStream::new(smuggle_te.as_bytes().to_vec())),
        Err(Error::Handshake(_))
    ));

    // Request smuggling: non-zero Content-Length on Upgrade request
    let smuggle_cl = concat!(
        "GET / HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Upgrade: websocket\r\n",
        "Connection: Upgrade\r\n",
        "Content-Length: 42\r\n",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Version: 13\r\n",
        "\r\n"
    );
    assert!(matches!(
        server::accept(MockStream::new(smuggle_cl.as_bytes().to_vec())),
        Err(Error::Handshake(_))
    ));

    // Duplicate Sec-WebSocket-Key
    let dup_key = concat!(
        "GET / HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Upgrade: websocket\r\n",
        "Connection: Upgrade\r\n",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Version: 13\r\n",
        "\r\n"
    );
    assert!(matches!(
        server::accept(MockStream::new(dup_key.as_bytes().to_vec())),
        Err(Error::Handshake(_))
    ));

    // Invalid key (wrong decoded length)
    let bad_key = concat!(
        "GET / HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Upgrade: websocket\r\n",
        "Connection: Upgrade\r\n",
        "Sec-WebSocket-Key: bm90MTZieXRlcw==\r\n",
        "Sec-WebSocket-Version: 13\r\n",
        "\r\n"
    );
    assert!(matches!(
        server::accept(MockStream::new(bad_key.as_bytes().to_vec())),
        Err(Error::Handshake(_))
    ));
}

#[test]
fn client_rejects_wss_and_header_crlf_injection() {
    assert!(matches!(
        client::connect("wss://example.com/ws"),
        Err(Error::UnsupportedScheme(s)) if s == "wss"
    ));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let err = ClientBuilder::new()
        .header("X-Evil", "ok\r\nInjected: yes")
        .connect(&format!("ws://{addr}/"))
        .unwrap_err();
    assert!(matches!(err, Error::Handshake(_)));
}

#[test]
fn client_rejects_mismatched_accept_key_and_non_101_status() {
    // 1) Mismatched Sec-WebSocket-Accept
    let listener1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr1 = listener1.local_addr().unwrap();
    let h1 = thread::spawn(move || {
        let (mut sock, _) = listener1.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf);
        let _ = sock.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\n\
              Upgrade: websocket\r\n\
              Connection: Upgrade\r\n\
              Sec-WebSocket-Accept: AAAAAAAAAAAAAAAAAAAAAAAAAAAA\r\n\r\n",
        );
    });
    assert!(matches!(
        client::connect(&format!("ws://{addr1}/")),
        Err(Error::Handshake(_))
    ));
    h1.join().unwrap();

    // 2) Non-101 HTTP status (403 Forbidden)
    let listener2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let h2 = thread::spawn(move || {
        let (mut sock, _) = listener2.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf);
        let _ = sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
    });
    assert!(matches!(
        client::connect(&format!("ws://{addr2}/")),
        Err(Error::HandshakeStatus { status: 403, .. })
    ));
    h2.join().unwrap();
}

#[test]
fn loopback_tcp_client_server_full_echo_and_close() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server_thread = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut ws = server::accept(tcp).unwrap();
        loop {
            match ws.recv().unwrap() {
                Message::Text(t) => ws.send_text(t).unwrap(),
                Message::Binary(b) => ws.send_binary(b).unwrap(),
                Message::Ping(_) => {
                    // auto_pong already replied
                }
                Message::Pong(_) => {}
                Message::Close(_) => break,
            }
        }
    });

    let mut ws = ClientBuilder::new()
        .timeout(Duration::from_secs(5))
        .connect(&format!("ws://{addr}/echo?v=1"))
        .unwrap();

    // Text echo
    ws.send_text("hello akku 🦀").unwrap();
    assert_eq!(ws.recv().unwrap(), Message::Text("hello akku 🦀".into()));

    // Large binary (> 65535 bytes -> 64-bit extended length framing)
    let big: Vec<u8> = (0..70_000).map(|i| (i % 251) as u8).collect();
    ws.send_binary(big.clone()).unwrap();
    assert_eq!(ws.recv().unwrap(), Message::Binary(big));

    // Ping -> server auto-pongs
    ws.ping(b"ping-payload".to_vec()).unwrap();
    assert_eq!(ws.recv().unwrap(), Message::Pong(b"ping-payload".to_vec()));

    // Clean close handshake
    ws.close(Some(CloseFrame::new(CloseCode::NORMAL, "done")))
        .unwrap();
    let close_reply = ws.recv().unwrap();
    assert!(matches!(
        close_reply,
        Message::Close(Some(cf)) if cf.code == CloseCode::NORMAL
    ));

    server_thread.join().unwrap();
}

#[test]
fn loopback_tcp_subprotocol_negotiation_and_rejection() {
    // 1) Successful negotiation via accept_hdr
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server_thread = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut ws = server::accept_hdr(tcp, Config::new(), |req| {
            assert_eq!(req.path, "/proto");
            assert_eq!(req.header("x-client-token"), Some("secret"));
            assert!(req.protocols.iter().any(|p| p == "chat.v1"));
            Ok(AcceptResponse::new()
                .with_protocol("chat.v1")
                .with_header("X-Server", "akku"))
        })
        .unwrap();
        assert_eq!(ws.protocol(), Some("chat.v1"));
        let msg = ws.recv().unwrap();
        ws.send(msg).unwrap();
    });

    let mut client_ws = ClientBuilder::new()
        .header("X-Client-Token", "secret")
        .protocol("json.v1")
        .protocol("chat.v1")
        .connect(&format!("ws://{addr}/proto"))
        .unwrap();
    assert_eq!(client_ws.protocol(), Some("chat.v1"));
    client_ws.send_text("negotiated").unwrap();
    assert_eq!(
        client_ws.recv().unwrap(),
        Message::Text("negotiated".into())
    );
    server_thread.join().unwrap();

    // 2) Rejection callback returns 401 Unauthorized
    let listener2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let server_thread2 = thread::spawn(move || {
        let (tcp, _) = listener2.accept().unwrap();
        let res = server::accept_hdr(tcp, Config::new(), |_| {
            Err(RejectResponse::new(401, "Unauthorized", "missing token"))
        });
        assert!(matches!(res, Err(Error::Handshake(_))));
    });

    let err = client::connect(&format!("ws://{addr2}/private")).unwrap_err();
    assert!(matches!(err, Error::HandshakeStatus { status: 401, .. }));
    server_thread2.join().unwrap();
}

#[test]
fn server_preserves_pipelined_frames_behind_upgrade_request() {
    // Client sends HTTP upgrade + masked Text("pipelined") in one contiguous buffer.
    let mut wire = Vec::new();
    wire.extend_from_slice(
        b"GET /pipe HTTP/1.1\r\n\
          Host: localhost\r\n\
          Upgrade: websocket\r\n\
          Connection: Upgrade\r\n\
          Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
          Sec-WebSocket-Version: 13\r\n\r\n",
    );
    wire.extend_from_slice(
        &Frame::text("pipelined")
            .with_mask([0xAA, 0xBB, 0xCC, 0xDD])
            .encode()
            .unwrap(),
    );

    let mut ws = server::accept(MockStream::new(wire)).unwrap();
    assert_eq!(ws.recv().unwrap(), Message::Text("pipelined".to_string()));
}

/// `Write` wrapper counting `write` calls (and collecting bytes) for coalescing tests.
#[derive(Debug, Default)]
struct CountingWriter {
    writes: usize,
    bytes: Vec<u8>,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for CountingWriter {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

#[test]
fn small_frames_are_written_with_a_single_write_call() {
    for frame in [
        Frame::text("Hello"),
        Frame::text("Hello").with_mask([0x37, 0xfa, 0x21, 0x3d]),
        Frame::binary(vec![7u8; 300]).with_mask([1, 2, 3, 4]),
        Frame::binary(vec![9u8; 40_000]),
        Frame::ping(Vec::new()),
    ] {
        let mut w = CountingWriter::default();
        frame.write_to(&mut w).unwrap();
        assert_eq!(w.writes, 1, "header and payload must be coalesced");
        assert_eq!(w.bytes, frame.encode().unwrap());
    }
}

#[test]
fn large_masked_frame_chunked_writes_round_trip() {
    let payload: Vec<u8> = (0..200_003u32).map(|i| (i % 253) as u8).collect();
    let frame = Frame::binary(payload.clone()).with_mask([0xde, 0xad, 0xbe, 0xef]);
    let mut w = CountingWriter::default();
    frame.write_to(&mut w).unwrap();
    // Header coalesced with the first chunk: ceil((14 + len) / 64 KiB) writes.
    assert_eq!(w.writes, (14 + payload.len()).div_ceil(64 * 1024));
    assert_eq!(w.bytes, frame.encode().unwrap());
    let decoded = Frame::read_from(&mut Cursor::new(w.bytes), usize::MAX).unwrap();
    assert_eq!(decoded.payload, payload);
}

#[test]
fn websocket_send_issues_one_write_per_small_message() {
    let mut ws = WebSocket::from_raw(CountingWriter::default(), Role::Client, Config::new());
    ws.send_text("hi").unwrap();
    assert_eq!(ws.get_ref().writes, 1);
    ws.send_binary(vec![1u8; 1000]).unwrap();
    assert_eq!(ws.get_ref().writes, 2);
}

fn client_mask_keys(ws: &WebSocket<CountingWriter>) -> Vec<[u8; 4]> {
    let mut cur = Cursor::new(ws.get_ref().bytes.clone());
    let mut keys = Vec::new();
    while (cur.position() as usize) < cur.get_ref().len() {
        let f = Frame::read_from(&mut cur, 1 << 20).unwrap();
        keys.push(f.mask.expect("client frames must be masked"));
    }
    keys
}

#[test]
fn client_mask_keys_differ_across_frames_and_connections() {
    let mut a = WebSocket::from_raw(CountingWriter::default(), Role::Client, Config::new());
    let mut b = WebSocket::from_raw(CountingWriter::default(), Role::Client, Config::new());
    for _ in 0..64 {
        a.send_text("x").unwrap();
        b.send_text("x").unwrap();
    }
    let ka = client_mask_keys(&a);
    let kb = client_mask_keys(&b);
    assert_eq!(ka.len(), 64);
    let mut all: Vec<[u8; 4]> = ka.iter().chain(kb.iter()).copied().collect();
    all.sort_unstable();
    all.dedup();
    // 128 random 32-bit keys: a collision is astronomically unlikely (~2^-19).
    assert!(all.len() >= 127, "mask keys must not repeat");
}

#[test]
fn generated_handshake_keys_are_unique() {
    let mut keys: Vec<String> = (0..256).map(|_| generate_key()).collect();
    assert!(keys.iter().all(|k| is_valid_key(k)));
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 256);
}
