//! Integration tests for `akku::sse` (WHATWG EventSource parser, encoder, client, server).

use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use akku::sse::client::{self, ClientBuilder};
use akku::sse::server::{SseResponse, SseWriter};
use akku::sse::{Error, Event, EventStream, Parser, DEFAULT_MAX_EVENT_BYTES};

#[test]
fn event_builder_wire_format_full() {
    let ev = Event::new()
        .comment("heartbeat")
        .event("update")
        .id("evt-1")
        .retry_ms(3000)
        .data("hello");

    assert_eq!(ev.event_type(), "update");
    assert_eq!(ev.event_name(), Some("update"));
    assert_eq!(ev.id_str(), Some("evt-1"));
    assert_eq!(ev.retry_duration(), Some(Duration::from_millis(3000)));
    assert_eq!(ev.data_str(), "hello");
    assert_eq!(ev.comments(), &["heartbeat"]);

    let wire = ev.try_to_wire_string().unwrap();
    assert_eq!(
        wire,
        ": heartbeat\nevent: update\nid: evt-1\nretry: 3000\ndata: hello\n\n"
    );
}

#[test]
fn event_multiline_data_and_empty_data() {
    let ev = Event::data_only("line1\nline2\r\nline3\rline4");
    assert_eq!(ev.event_type(), "message");
    assert_eq!(
        ev.try_to_wire_string().unwrap(),
        "data: line1\ndata: line2\ndata: line3\ndata: line4\n\n"
    );

    let empty = Event::data_only("");
    assert_eq!(empty.try_to_wire_string().unwrap(), "data:\n\n");

    let blanks = Event::data_only("a\n\nb\n");
    assert_eq!(
        blanks.try_to_wire_string().unwrap(),
        "data: a\ndata:\ndata: b\ndata:\n\n"
    );

    // Roundtrip through Parser preserves exact newlines
    let mut parser = Parser::new();
    let parsed = parser
        .push(blanks.try_to_wire_string().unwrap().as_bytes())
        .unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].data_str(), "a\n\nb\n");
}

#[test]
fn event_leading_space_roundtrips_losslessly() {
    let ev = Event::data_only("  two leading spaces");
    let wire = ev.try_to_wire_string().unwrap();
    let mut parser = Parser::new();
    let parsed = parser.push(wire.as_bytes()).unwrap();
    assert_eq!(parsed[0].data_str(), "  two leading spaces");
}

#[test]
fn event_strict_rejects_crlf_and_nul_injection() {
    let bad_event = Event::new().event("ok\r\ndata: injected").data("x");
    assert!(matches!(
        bad_event.try_to_wire_string(),
        Err(Error::InvalidField { field: "event", .. })
    ));

    let bad_id_lf = Event::new().id("1\ndata: injected").data("x");
    assert!(matches!(
        bad_id_lf.try_to_wire_string(),
        Err(Error::InvalidField { field: "id", .. })
    ));

    let bad_id_nul = Event::new().id("1\0a").data("x");
    assert!(matches!(
        bad_id_nul.try_to_wire_string(),
        Err(Error::InvalidField { field: "id", .. })
    ));
}

#[test]
fn event_to_wire_string_sanitizes_injection() {
    let ev = Event::new()
        .event("notice\r\ndata: evil")
        .id("id1\n\0data: evil")
        .data("legit");
    let wire = ev.to_wire_string();
    assert!(!wire.contains("\ndata: evil\n"));

    let mut parser = Parser::new();
    let parsed = parser.push(wire.as_bytes()).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].data_str(), "legit");
}

#[test]
fn parser_strips_utf8_bom_once_including_split_chunks() {
    // Single chunk with BOM
    let mut p1 = Parser::new();
    let input = b"\xEF\xBB\xBFdata: bom-ok\n\n";
    let evs = p1.push(input).unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].data_str(), "bom-ok");

    // BOM split 1 byte at a time across chunks
    let mut p2 = Parser::new();
    let mut all = Vec::new();
    for b in input {
        all.extend(p2.push(&[*b]).unwrap());
    }
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].data_str(), "bom-ok");

    // Second BOM is NOT stripped (part of field name -> unknown field, ignored)
    let mut p3 = Parser::new();
    let double_bom = b"\xEF\xBB\xBF\xEF\xBB\xBFdata: second\n\ndata: real\n\n";
    let evs3 = p3.push(double_bom).unwrap();
    assert_eq!(evs3.len(), 1);
    assert_eq!(evs3[0].data_str(), "real");
}

#[test]
fn parser_handles_partial_bom_prefix_without_dropping_bytes() {
    // Starts with 0xEF 0xBB then non-0xBF byte (not a BOM). Must not drop bytes or panic.
    let mut parser = Parser::new();
    let mut evs = Vec::new();
    evs.extend(parser.push(&[0xEF]).unwrap());
    evs.extend(parser.push(&[0xBB]).unwrap());
    evs.extend(parser.push(b"x\n\ndata: after\n\n").unwrap());
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].data_str(), "after");
}

#[test]
fn parser_handles_lf_crlf_and_cr_byte_by_byte() {
    let stream = b"data: lf\n\ndata: crlf\r\n\r\ndata: cr\r\rdata: mixed\r\n\r";
    let mut parser = Parser::new();
    let mut events = Vec::new();
    for &b in stream {
        events.extend(parser.push(&[b]).unwrap());
    }
    let datas: Vec<&str> = events.iter().map(Event::data_str).collect();
    assert_eq!(datas, vec!["lf", "crlf", "cr", "mixed"]);
}

#[test]
fn parser_crlf_split_across_chunk_boundary_is_single_eol() {
    // If \r and \n across chunks were mistakenly treated as TWO line endings,
    // the event would dispatch early after line1 instead of joining line1 + line2!
    let mut parser = Parser::new();
    assert!(parser.push(b"data: line1\r").unwrap().is_empty());
    assert!(parser.push(b"\ndata: line2\r").unwrap().is_empty());
    let evs = parser.push(b"\n\r\n").unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].data_str(), "line1\nline2");
}

#[test]
fn parser_leading_space_rule_colon_and_fieldless_lines() {
    let input = concat!(
        "data:  two\n",    // strips ONE space -> " two"
        "data:no_space\n", // no space -> "no_space"
        "data\n",          // no colon -> empty value ""
        "data:\n",         // empty after colon -> ""
        "data: \n",        // single space stripped -> ""
        "\n"
    );
    let mut parser = Parser::new();
    let evs = parser.push(input.as_bytes()).unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].data_str(), " two\nno_space\n\n\n");
}

#[test]
fn parser_ignores_comments_and_unknown_fields() {
    let input = concat!(
        ": comment 1\n",
        ":\n",
        "unknown: value\n",
        "customfield\n",
        "data: payload\n",
        ": mid-event comment\n",
        "\n"
    );
    let mut parser = Parser::new();
    let evs = parser.push(input.as_bytes()).unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].event_type(), "message");
    assert_eq!(evs[0].data_str(), "payload");
}

#[test]
fn parser_blank_line_without_data_clears_event_name_and_emits_nothing() {
    let input = concat!(
        "event: custom\n",
        "id: sticky-1\n",
        "\n",
        "data: next\n",
        "\n"
    );
    let mut parser = Parser::new();
    let evs = parser.push(input.as_bytes()).unwrap();
    assert_eq!(evs.len(), 1);
    // event name was cleared by the blank line, so default "message" applies
    assert_eq!(evs[0].event_type(), "message");
    assert_eq!(evs[0].event_name(), None);
    // id persisted per WHATWG spec!
    assert_eq!(evs[0].id_str(), Some("sticky-1"));
    assert_eq!(evs[0].data_str(), "next");
}

#[test]
fn parser_last_event_id_persistence_reset_and_nul_ignore() {
    let input = concat!(
        "id: 100\n",
        "data: first\n\n",
        "data: second\n\n", // inherits id 100
        "id: bad\0id\n",    // NUL -> MUST be ignored per WHATWG
        "data: third\n\n",  // still inherits 100
        "id:\n",            // empty id resets last_event_id to ""
        "data: fourth\n\n",
        "id\n", // fieldless id also resets to ""
        "data: fifth\n\n"
    );
    let mut parser = Parser::new();
    let evs = parser.push(input.as_bytes()).unwrap();
    assert_eq!(evs.len(), 5);
    assert_eq!(evs[0].id_str(), Some("100"));
    assert_eq!(evs[1].id_str(), Some("100"));
    assert_eq!(evs[2].id_str(), Some("100"));
    assert_eq!(evs[3].id_str(), None);
    assert_eq!(evs[4].id_str(), None);
    assert_eq!(parser.last_event_id(), "");
}

#[test]
fn parser_retry_valid_and_invalid_values() {
    let input = concat!(
        "retry: 2500\n",
        "data: one\n\n",
        "retry: 100ms\n",                      // non-digit -> ignored
        "retry: -50\n",                        // sign -> ignored
        "retry: \n",                           // empty -> ignored
        "retry: 99999999999999999999999999\n", // u64 overflow -> ignored
        "data: two\n\n",
        "retry: 0\n", // valid 0ms
        "data: three\n\n"
    );
    let mut parser = Parser::new();
    let evs = parser.push(input.as_bytes()).unwrap();
    assert_eq!(evs.len(), 3);
    assert_eq!(evs[0].retry_duration(), Some(Duration::from_millis(2500)));
    assert_eq!(evs[1].retry_duration(), None);
    assert_eq!(evs[2].retry_duration(), Some(Duration::from_millis(0)));
    assert_eq!(parser.retry(), Some(Duration::from_millis(0)));
}

#[test]
fn parser_eof_discards_incomplete_trailing_event() {
    let mut parser = Parser::new();
    let evs = parser
        .push(b"data: complete\n\ndata: incomplete\n")
        .unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].data_str(), "complete");
    parser.finish();
    // Feeding a blank line after finish must not resurrect the discarded event
    assert!(parser.push(b"\n").unwrap().is_empty());
}

#[test]
fn parser_push_line_api() {
    let mut parser = Parser::new();
    assert!(parser.push_line("\u{FEFF}event: ping").unwrap().is_none());
    assert!(parser.push_line("data: ok").unwrap().is_none());
    let ev = parser.push_line("").unwrap().unwrap();
    assert_eq!(ev.event_type(), "ping");
    assert_eq!(ev.data_str(), "ok");
}

#[test]
fn parser_dos_cap_single_overlong_line() {
    let mut parser = Parser::with_max_event_bytes(64);
    assert_eq!(parser.max_event_bytes(), 64);
    let huge_line = vec![b'a'; 100];
    assert!(matches!(
        parser.push(&huge_line),
        Err(Error::EventTooLarge { max: 64 })
    ));
}

#[test]
fn parser_dos_cap_accumulated_multiline_event() {
    let mut parser = Parser::with_max_event_bytes(32);
    assert!(parser.push(b"data: 0123456789\n").unwrap().is_empty());
    assert!(parser.push(b"data: 0123456789\n").unwrap().is_empty());
    // Third data line pushes accumulated buffer over 32 bytes
    assert!(matches!(
        parser.push(b"data: 01234567890123456789\n"),
        Err(Error::EventTooLarge { max: 32 })
    ));
}

#[test]
fn parser_utf8_multibyte_split_across_chunks() {
    // "🚀" is [0xF0, 0x9F, 0x9A, 0x80]; split inside push() chunks
    let wire = "data: launch 🚀\n\n".as_bytes();
    let mut parser = Parser::new();
    let mut evs = Vec::new();
    for chunk in wire.chunks(2) {
        evs.extend(parser.push(chunk).unwrap());
    }
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].data_str(), "launch 🚀");
}

#[test]
fn event_stream_iterator_and_metadata() {
    assert_eq!(DEFAULT_MAX_EVENT_BYTES, 1024 * 1024);
    let raw = "id: 1\nretry: 500\ndata: alpha\n\nevent: done\ndata: beta\n\n";
    let mut stream = EventStream::new(Cursor::new(raw.as_bytes()));
    let collected: Vec<Event> = stream.by_ref().map(|r| r.unwrap()).collect();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0].data_str(), "alpha");
    assert_eq!(collected[1].event_type(), "done");
    assert_eq!(collected[1].data_str(), "beta");
    assert_eq!(stream.last_event_id(), "1");
    assert_eq!(stream.retry(), Some(Duration::from_millis(500)));
}

#[test]
fn sse_writer_raw_and_chunked_encoding() {
    // Raw mode
    let mut raw_out = Vec::new();
    {
        let mut w = SseWriter::new(&mut raw_out);
        w.send_comment("keep-alive").unwrap();
        w.send_retry(Duration::from_millis(1500)).unwrap();
        w.send_data("hello").unwrap();
        w.send(&Event::new().event("e").id("9").data("world"))
            .unwrap();
    }
    let raw_str = String::from_utf8(raw_out).unwrap();
    assert_eq!(
        raw_str,
        ": keep-alive\n\nretry: 1500\n\ndata: hello\n\nevent: e\nid: 9\ndata: world\n\n"
    );

    // Chunked HTTP mode
    let mut chunked_out = Vec::new();
    {
        let mut w = SseWriter::begin_http_chunked(&mut chunked_out).unwrap();
        w.send_data("hi").unwrap();
        w.finish().unwrap();
    }
    let chunked_str = String::from_utf8(chunked_out).unwrap();
    assert!(chunked_str.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(chunked_str.contains("Transfer-Encoding: chunked\r\n"));
    assert!(chunked_str.ends_with("0\r\n\r\n"));
}

#[test]
fn sse_response_builder_and_header_validation() {
    let resp = SseResponse::new()
        .comment("start")
        .retry(Duration::from_millis(2000))
        .event(&Event::new().id("1").data("a"))
        .unwrap()
        .events([&Event::data_only("b")])
        .unwrap()
        .header("X-Stream", "1")
        .unwrap();

    assert_eq!(
        resp.as_wire_str(),
        ": start\n\nretry: 2000\n\nid: 1\ndata: a\n\ndata: b\n\n"
    );

    // Rejects CRLF in header value
    assert!(matches!(
        SseResponse::new().header("X-Bad", "ok\r\nEvil: 1"),
        Err(Error::InvalidField {
            field: "header",
            ..
        })
    ));
}

#[test]
fn loopback_tcp_client_server_raw_stream_and_last_event_id() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server_thread = thread::spawn(move || {
        let (mut tcp, _) = listener.accept().unwrap();
        let mut req_buf = [0u8; 2048];
        let n = tcp.read(&mut req_buf).unwrap();
        let req_text = String::from_utf8_lossy(&req_buf[..n]);
        assert!(req_text.contains("Accept: text/event-stream\r\n"));
        assert!(req_text.contains("Last-Event-ID: evt-0\r\n"));
        assert!(req_text.contains("X-Token: abc\r\n"));

        let mut writer = SseWriter::begin_http(tcp).unwrap();
        writer.send_comment("connected").unwrap();
        writer
            .send(&Event::new().id("evt-1").event("tick").data("one"))
            .unwrap();
        writer
            .send(&Event::new().id("evt-2").data("two\nlines"))
            .unwrap();
        let _ = writer.finish().unwrap();
    });

    let mut stream = ClientBuilder::new()
        .timeout(Duration::from_secs(5))
        .last_event_id("evt-0")
        .header("X-Token", "abc")
        .connect(&format!("http://{addr}/events?channel=main"))
        .unwrap();

    assert_eq!(stream.status(), 200);
    assert_eq!(
        stream.header("content-type"),
        Some("text/event-stream; charset=utf-8")
    );

    let e1 = stream.next_event().unwrap().unwrap();
    assert_eq!(e1.event_type(), "tick");
    assert_eq!(e1.id_str(), Some("evt-1"));
    assert_eq!(e1.data_str(), "one");

    let e2 = stream.next_event().unwrap().unwrap();
    assert_eq!(e2.event_type(), "message");
    assert_eq!(e2.id_str(), Some("evt-2"));
    assert_eq!(e2.data_str(), "two\nlines");
    assert_eq!(stream.last_event_id(), "evt-2");

    assert!(stream.next_event().unwrap().is_none());
    server_thread.join().unwrap();
}

#[test]
fn loopback_tcp_client_server_chunked_transfer_encoding() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server_thread = thread::spawn(move || {
        let (mut tcp, _) = listener.accept().unwrap();
        let mut req_buf = [0u8; 1024];
        let _ = tcp.read(&mut req_buf);

        let mut writer = SseWriter::begin_http_chunked(tcp).unwrap();
        writer.send_retry(Duration::from_millis(750)).unwrap();
        writer
            .send(&Event::new().id("c1").data("chunked-1"))
            .unwrap();
        writer
            .send(&Event::new().id("c2").data("chunked-2"))
            .unwrap();
        let _ = writer.finish().unwrap();
    });

    let stream = client::connect(&format!("http://{addr}/chunked")).unwrap();
    let events: Vec<Event> = stream.map(|r| r.unwrap()).collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data_str(), "chunked-1");
    assert_eq!(events[0].id_str(), Some("c1"));
    assert_eq!(events[1].data_str(), "chunked-2");
    assert_eq!(events[1].id_str(), Some("c2"));

    server_thread.join().unwrap();
}

#[test]
fn loopback_tcp_chunked_with_extensions_and_split_event_across_chunks() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server_thread = thread::spawn(move || {
        let (mut tcp, _) = listener.accept().unwrap();
        let mut req_buf = [0u8; 1024];
        let _ = tcp.read(&mut req_buf);

        // Manually split a single SSE event across two HTTP chunks with chunk extensions & trailer
        let resp = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: text/event-stream\r\n",
            "Transfer-Encoding: chunked\r\n\r\n",
            "8;ext=1\r\n",
            "data: he\r\n",
            "5\r\n",
            "llo\n\n\r\n",
            "0\r\n",
            "X-Trailer: done\r\n",
            "\r\n"
        );
        tcp.write_all(resp.as_bytes()).unwrap();
    });

    let mut stream = client::connect(&format!("http://{addr}/split")).unwrap();
    let ev = stream.next_event().unwrap().unwrap();
    assert_eq!(ev.data_str(), "hello");
    assert!(stream.next_event().unwrap().is_none());

    server_thread.join().unwrap();
}

#[test]
fn client_rejects_https_bad_status_bad_content_type_and_crlf_headers() {
    // https unsupported without TLS
    assert!(matches!(
        client::connect("https://example.com/events"),
        Err(Error::UnsupportedScheme(s)) if s == "https"
    ));

    // CRLF in Last-Event-ID
    assert!(matches!(
        ClientBuilder::new()
            .last_event_id("1\r\nX-Injected: yes")
            .connect("http://127.0.0.1:1/"),
        Err(Error::InvalidField {
            field: "Last-Event-ID",
            ..
        })
    ));

    // Non-200 status (503)
    let listener1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr1 = listener1.local_addr().unwrap();
    let h1 = thread::spawn(move || {
        let (mut tcp, _) = listener1.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = tcp.read(&mut buf);
        let _ = tcp.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 4\r\n\r\nbusy");
    });
    assert!(matches!(
        client::connect(&format!("http://{addr1}/")),
        Err(Error::HttpStatus { status: 503, body }) if body == "busy"
    ));
    h1.join().unwrap();

    // Wrong Content-Type
    let listener2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let h2 = thread::spawn(move || {
        let (mut tcp, _) = listener2.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = tcp.read(&mut buf);
        let _ = tcp.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
        );
    });
    assert!(matches!(
        client::connect(&format!("http://{addr2}/")),
        Err(Error::InvalidContentType(ct)) if ct == "text/plain"
    ));
    h2.join().unwrap();
}

#[cfg(feature = "http")]
#[test]
fn http_feature_sse_response_and_event_into_response_roundtrip() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let (addr, server_handle) = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = akku::http::App::new()
            .get("/stream", || async {
                SseResponse::new()
                    .event(&Event::new().event("hello").id("1").data("from-axum"))
                    .unwrap()
                    .event(&Event::data_only("second"))
                    .unwrap()
            })
            .get("/single", || async { Event::data_only("single-event") });
        let handle = tokio::spawn(async move {
            let _ = app.serve_listener(listener).await;
        });
        (addr, handle)
    });

    let client_thread = thread::spawn(move || {
        let mut s1 = client::connect(&format!("http://{addr}/stream")).unwrap();
        assert_eq!(
            s1.header("content-type"),
            Some("text/event-stream; charset=utf-8")
        );
        assert_eq!(s1.header("cache-control"), Some("no-cache"));
        let evs1: Vec<Event> = s1.by_ref().map(|r| r.unwrap()).collect();
        assert_eq!(evs1.len(), 2);
        assert_eq!(evs1[0].event_type(), "hello");
        assert_eq!(evs1[0].data_str(), "from-axum");
        assert_eq!(evs1[1].data_str(), "second");

        let s2 = client::connect(&format!("http://{addr}/single")).unwrap();
        let evs2: Vec<Event> = s2.map(|r| r.unwrap()).collect();
        assert_eq!(evs2.len(), 1);
        assert_eq!(evs2[0].data_str(), "single-event");
    });

    rt.block_on(async {
        tokio::task::spawn_blocking(move || client_thread.join().unwrap())
            .await
            .unwrap();
        server_handle.abort();
    });
}

const INJ_LIMIT: usize = 64;

fn injection_wire() -> Vec<u8> {
    let mut wire = b"data: a\n\n".to_vec();
    wire.extend_from_slice(b"data: ");
    wire.resize(wire.len() + INJ_LIMIT, b'x');
    wire.extend_from_slice(b"event: evil\ndata: injected\n\n");
    wire.extend_from_slice(b"data: b\n\n");
    wire
}

/// Feeds `chunks` and drains every queued result, returning them in stream order.
fn collect_results(parser: &mut Parser, chunks: &[&[u8]]) -> Vec<Result<Event, Error>> {
    let mut out = Vec::new();
    let drain = |parser: &mut Parser, chunk: &[u8], out: &mut Vec<Result<Event, Error>>| {
        match parser.push(chunk) {
            Ok(evs) => out.extend(evs.into_iter().map(Ok)),
            Err(e) => out.push(Err(e)),
        }
        while parser.has_pending() {
            match parser.push(&[]) {
                Ok(evs) => out.extend(evs.into_iter().map(Ok)),
                Err(e) => out.push(Err(e)),
            }
        }
    };
    for chunk in chunks {
        drain(parser, chunk, &mut out);
    }
    out
}

fn assert_injection_outcome(results: &[Result<Event, Error>]) {
    assert_eq!(results.len(), 3, "expected a, error, b: {results:?}");
    assert_eq!(results[0].as_ref().unwrap().data_str(), "a");
    assert!(matches!(
        results[1],
        Err(Error::EventTooLarge { max: INJ_LIMIT })
    ));
    let b = results[2].as_ref().unwrap();
    assert_eq!(b.data_str(), "b");
    assert_eq!(b.event_type(), "message");
    for ev in results.iter().flatten() {
        assert_ne!(ev.event_type(), "evil");
        assert!(!ev.data_str().contains("injected"));
    }
}

#[test]
fn parser_oversized_line_cannot_inject_fields_single_chunk() {
    let wire = injection_wire();
    let mut parser = Parser::with_max_event_bytes(INJ_LIMIT);
    // First call yields the event completed before the overflow.
    let first = parser.push(&wire).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].data_str(), "a");
    assert!(parser.has_pending());
    assert!(matches!(
        parser.push(&[]),
        Err(Error::EventTooLarge { max: INJ_LIMIT })
    ));
    let rest = parser.push(&[]).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].data_str(), "b");
    assert!(!parser.has_pending());
    assert!(parser.push(&[]).unwrap().is_empty());
}

#[test]
fn parser_oversized_line_cannot_inject_fields_any_split() {
    let wire = injection_wire();
    for split in 0..=wire.len() {
        let (left, right) = wire.split_at(split);
        let mut parser = Parser::with_max_event_bytes(INJ_LIMIT);
        let results = collect_results(&mut parser, &[left, right]);
        assert_injection_outcome(&results);
    }
    // Byte-at-a-time feeding.
    let mut parser = Parser::with_max_event_bytes(INJ_LIMIT);
    let chunks: Vec<&[u8]> = wire.chunks(1).collect();
    assert_injection_outcome(&collect_results(&mut parser, &chunks));
}

#[test]
fn parser_oversized_line_with_crlf_endings() {
    let wire = String::from_utf8(injection_wire())
        .unwrap()
        .replace('\n', "\r\n");
    for split in 0..=wire.len() {
        let (left, right) = wire.as_bytes().split_at(split);
        let mut parser = Parser::with_max_event_bytes(INJ_LIMIT);
        assert_injection_outcome(&collect_results(&mut parser, &[left, right]));
    }
}

#[test]
fn parser_budget_overflow_drops_rest_of_event() {
    let mut parser = Parser::with_max_event_bytes(32);
    let wire = b"data: 0123456789\ndata: 0123456789\ndata: 01234567890123456789\nevent: evil\ndata: tail\n\ndata: ok\n\n";
    let results = collect_results(&mut parser, &[wire]);
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(matches!(results[0], Err(Error::EventTooLarge { max: 32 })));
    let ok = results[1].as_ref().unwrap();
    assert_eq!(ok.data_str(), "ok");
    assert_eq!(ok.event_type(), "message");
}

#[test]
fn push_line_discards_rest_of_oversized_event() {
    let mut parser = Parser::with_max_event_bytes(8);
    assert!(parser.push_line("data: 0123456789").is_err());
    assert!(parser.push_line("event: evil").unwrap().is_none());
    assert!(parser.push_line("data: injected").unwrap().is_none());
    assert!(parser.push_line("").unwrap().is_none());
    assert!(parser.push_line("data: b").unwrap().is_none());
    let ev = parser.push_line("").unwrap().unwrap();
    assert_eq!(ev.data_str(), "b");
    assert_eq!(ev.event_type(), "message");
}

#[test]
fn event_stream_yields_events_around_oversized_event_in_order() {
    let wire = injection_wire();
    let stream = EventStream::with_max_event_bytes(Cursor::new(wire), INJ_LIMIT);
    let results: Vec<_> = stream.collect();
    assert_injection_outcome(&results);
}

#[test]
fn parser_coalesces_flood_of_oversized_events() {
    let mut parser = Parser::with_max_event_bytes(16);
    let mut wire = Vec::new();
    for _ in 0..1000 {
        wire.extend_from_slice(b"data: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n\n");
    }
    wire.extend_from_slice(b"data: ok\n\n");
    // One coalesced error for the whole flood, then the valid event.
    assert!(matches!(
        parser.push(&wire),
        Err(Error::EventTooLarge { max: 16 })
    ));
    let rest = parser.push(&[]).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].data_str(), "ok");
    assert!(!parser.has_pending());

    // Accumulated-budget overflows are coalesced too.
    let mut parser = Parser::with_max_event_bytes(16);
    let wire = b"data: 0123456789\ndata: 0123456789\n\n".repeat(1000);
    assert!(parser.push(&wire).is_err());
    assert!(parser.push(&[]).unwrap().is_empty());
    assert!(!parser.has_pending());
}

#[test]
fn parser_last_event_id_commits_on_dispatch_only() {
    let mut parser = Parser::new();
    assert!(parser.push(b"id: 1\ndata: a\n\n").unwrap().len() == 1);
    assert_eq!(parser.last_event_id(), "1");
    // Pending id is not committed before the blank line.
    assert!(parser.push(b"id: 2\ndata: b\n").unwrap().is_empty());
    assert_eq!(parser.last_event_id(), "1");
    // EOF drops the incomplete event and its id.
    parser.finish();
    assert_eq!(parser.last_event_id(), "1");
    let evs = parser.push(b"data: c\n\n").unwrap();
    assert_eq!(evs[0].id_str(), Some("1"));
    // An id-only event still commits per WHATWG dispatch step 1.
    assert!(parser.push(b"id: 3\n\n").unwrap().is_empty());
    assert_eq!(parser.last_event_id(), "3");
}

#[test]
fn parser_oversized_event_does_not_advance_last_event_id() {
    let mut parser = Parser::with_max_event_bytes(16);
    assert_eq!(parser.push(b"id: 1\ndata: a\n\n").unwrap().len(), 1);
    // Oversized data after the id: the whole event (incl. its id) is dropped.
    assert!(parser
        .push(b"id: 9\ndata: xxxxxxxxxxxxxxxxxxxxxxxx\n\n")
        .is_err());
    assert_eq!(parser.last_event_id(), "1");
    let evs = parser.push(b"data: b\n\n").unwrap();
    assert_eq!(evs[0].id_str(), Some("1"));
    assert_eq!(parser.last_event_id(), "1");

    // Same via an over-budget accumulated event and the push_line API.
    let mut parser = Parser::with_max_event_bytes(16);
    assert!(parser.push_line("id: 7").unwrap().is_none());
    assert!(parser.push_line("data: 0123456").unwrap().is_none());
    assert!(parser.push_line("data: 01234567").is_err());
    assert!(parser.push_line("").unwrap().is_none());
    assert_eq!(parser.last_event_id(), "");
    assert!(parser.push_line("data: z").unwrap().is_none());
    assert_eq!(parser.push_line("").unwrap().unwrap().id_str(), None);
}
