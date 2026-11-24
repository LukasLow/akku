#![cfg(feature = "http")]

use akku::html::Html;
use akku::http::{App, Path};

#[akku::http::get("/")]
async fn home() -> Html {
    Html::new("<h1>Hello</h1>")
}

#[akku::http::get("/users/{id}")]
async fn user(Path(id): Path<u64>) -> Html {
    Html::new(format!("<h1>User {id}</h1>"))
}

#[test]
fn attribute_routes_mount_into_an_app() {
    let app = App::new().mount(home).mount(user);
    let _router = app.into_axum();
}

#[test]
fn client_rejects_whitespace_and_control_bytes_in_request_target() {
    use akku::http::client::{Client, ClientError};

    let client = Client::new();
    for bad in [
        "http://127.0.0.1:80/hello world",
        "http://127.0.0.1:80/hello\tworld",
        "http://127.0.0.1:80/hello?q=a b",
        "http://127.0.0.1:80/hello\x7f",
        "http://127.0.0.1:80/café",
    ] {
        let err = client.get(bad).send().unwrap_err();
        assert!(
            matches!(err, ClientError::InvalidUrl(_)),
            "expected InvalidUrl for {bad:?}, got {err:?}"
        );
    }
}

#[test]
fn client_resolves_relative_redirect_with_scheme_in_query() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use akku::http::client::Client;
    use akku::http::StatusCode;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        // First request: respond with 302 Location: /login?next=http://example.com
        let (mut s1, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = s1.read(&mut buf);
        s1.write_all(
            b"HTTP/1.1 302 Found\r\nLocation: /login?next=http://example.com\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap();

        // Second request: must hit the same host with path `/login?next=http://example.com`.
        let (mut s2, _) = listener.accept().unwrap();
        let mut req2 = [0u8; 1024];
        let n = s2.read(&mut req2).unwrap();
        let req2_str = String::from_utf8_lossy(&req2[..n]);
        assert!(
            req2_str.starts_with("GET /login?next=http://example.com HTTP/1.1\r\n"),
            "unexpected second request: {req2_str:?}"
        );
        s2.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .unwrap();
    });

    let res = Client::new()
        .get(format!("http://{addr}/start"))
        .send()
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().unwrap(), "ok");
    handle.join().unwrap();
}

#[test]
fn client_rejects_oversized_response_bodies() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use akku::http::client::{Client, ClientError};

    let client = Client::builder().max_response_body_bytes(8).build();

    // 1. Oversized Content-Length (server does not even need to send payload).
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf);
        let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n");
    });
    let err = client.get(format!("http://{addr}/cl")).send().unwrap_err();
    assert!(matches!(err, ClientError::InvalidResponse(_)), "{err:?}");
    handle.join().unwrap();

    // 2. Oversized chunked body.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf);
        let _ = sock.write_all(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\n123456789\r\n0\r\n\r\n",
        );
    });
    let err = client
        .get(format!("http://{addr}/chunked"))
        .send()
        .unwrap_err();
    assert!(matches!(err, ClientError::InvalidResponse(_)), "{err:?}");
    handle.join().unwrap();

    // 3. Oversized EOF-delimited body.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf);
        let _ = sock.write_all(b"HTTP/1.0 200 OK\r\n\r\n123456789");
    });
    let err = client.get(format!("http://{addr}/eof")).send().unwrap_err();
    assert!(matches!(err, ClientError::InvalidResponse(_)), "{err:?}");
    handle.join().unwrap();
}

#[test]
fn client_rejects_excessive_1xx_informational_responses() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use akku::http::client::{Client, ClientError};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf);
        for _ in 0..20 {
            if sock.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
                break;
            }
        }
    });

    let err = Client::new()
        .get(format!("http://{addr}/info-loop"))
        .send()
        .unwrap_err();
    assert!(matches!(err, ClientError::InvalidResponse(_)), "{err:?}");
    handle.join().unwrap();
}
