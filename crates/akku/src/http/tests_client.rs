#![cfg(feature = "http")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use akku::http::client::{Client, ClientError};
use akku::http::{
    ok_html, ok_json_raw, ok_text, redirect, status_text, App, Headers, Method, Path, Request,
    Response, ResponseBuilder, StatusCode,
};
use akku::test::TestServer;

#[akku::http::get("/hello/{name}")]
async fn hello(Path(name): Path<String>) -> Response {
    ok_text(format!("hello {name}"))
}

#[akku::http::post("/echo")]
async fn echo_post(req: Request) -> Response {
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    ResponseBuilder::new(StatusCode::CREATED)
        .header("x-seen-ct", ct)
        .text(format!("POST:{body}"))
}

#[akku::http::put("/items/{id}")]
async fn update_item(Path(id): Path<u64>, req: Request) -> Response {
    let bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    ok_text(format!("PUT:{id}:{body}"))
}

#[akku::http::patch("/items/{id}")]
async fn patch_item(Path(id): Path<u64>, req: Request) -> Response {
    let bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    ok_text(format!("PATCH:{id}:{body}"))
}

#[akku::http::delete("/items/{id}")]
async fn delete_item(Path(id): Path<u64>) -> Response {
    ResponseBuilder::new(StatusCode::OK).text(format!("DELETE:{id}"))
}

#[akku::http::get("/inspect")]
async fn inspect_req(req: Request) -> Response {
    let query = req.uri().query().unwrap_or("").to_string();
    let ua = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let x_default = req
        .headers()
        .get("x-default")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let tags: Vec<String> = req
        .headers()
        .get_all("x-tag")
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect();

    ResponseBuilder::new(StatusCode::OK)
        .header("x-resp-multi", "first")
        .header("x-resp-multi", "second")
        .text(format!(
            "query={query}|ua={ua}|default={x_default}|tags={}",
            tags.join(",")
        ))
}

#[akku::http::get("/html")]
async fn html_page() -> Response {
    ok_html("<p>hi</p>")
}

#[akku::http::get("/json")]
async fn json_page() -> Response {
    ok_json_raw(r#"{"ok":true}"#)
}

#[akku::http::post("/redir-302")]
async fn redir_302() -> Response {
    redirect(StatusCode::FOUND, "/redir-target")
}

#[akku::http::post("/redir-307")]
async fn redir_307() -> Response {
    redirect(StatusCode::TEMPORARY_REDIRECT, "/echo")
}

#[akku::http::get("/nested/start")]
async fn redir_relative() -> Response {
    redirect(StatusCode::SEE_OTHER, "../redir-target")
}

#[akku::http::get("/redir-target")]
async fn redir_target() -> Response {
    ok_text("landed-get")
}

#[akku::http::get("/loop-a")]
async fn loop_a() -> Response {
    redirect(StatusCode::FOUND, "/loop-b")
}

#[akku::http::get("/loop-b")]
async fn loop_b() -> Response {
    redirect(StatusCode::FOUND, "/loop-a")
}

#[akku::http::get("/missing")]
async fn missing() -> Response {
    status_text(StatusCode::NOT_FOUND, "no such item")
}

#[test]
fn core_http_types_method_status_and_headers() {
    assert_eq!(Method::Get.as_str(), "GET");
    assert_eq!("post".parse::<Method>().unwrap(), Method::Post);
    assert_eq!(
        "PROPFIND".parse::<Method>().unwrap(),
        Method::Custom("PROPFIND".to_string())
    );
    assert_eq!(Method::Custom("LOCK".to_string()).to_string(), "LOCK");
    assert!("BAD METHOD".parse::<Method>().is_err());
    assert!("".parse::<Method>().is_err());

    assert_eq!(StatusCode::OK.as_u16(), 200);
    assert!(StatusCode::OK.is_success());
    assert!(StatusCode::FOUND.is_redirection());
    assert!(StatusCode::NOT_FOUND.is_client_error());
    assert!(StatusCode::INTERNAL_SERVER_ERROR.is_server_error());
    assert_eq!(StatusCode::NOT_FOUND.canonical_reason(), Some("Not Found"));
    assert_eq!(StatusCode::OK.to_string(), "200 OK");
    assert_eq!(StatusCode::from_u16(99), None);
    assert_eq!(StatusCode::from_u16(299).unwrap().to_string(), "299");

    let mut headers = Headers::new();
    assert!(headers.is_empty());
    headers.append("X-Foo", "one");
    headers.append("x-foo", "two");
    headers.insert("Content-Type", "text/plain");
    assert_eq!(headers.len(), 3);
    assert!(headers.contains("X-FOO"));
    assert_eq!(headers.get("x-foo"), Some("one"));
    assert_eq!(headers.get_all("X-FOO"), vec!["one", "two"]);

    headers.insert("x-foo", "replaced");
    assert_eq!(headers.len(), 2);
    assert_eq!(headers.get_all("x-foo"), vec!["replaced"]);
    let pairs: Vec<_> = headers.iter().collect();
    assert_eq!(
        pairs,
        vec![("x-foo", "replaced"), ("Content-Type", "text/plain")]
    );
    let removed = headers.remove("X-FOO");
    assert_eq!(removed, vec!["replaced".to_string()]);
    assert!(!headers.contains("x-foo"));
}

#[test]
fn server_routing_nest_merge_and_client_crud_headers_query() {
    let nested_api = App::new()
        .mount(update_item)
        .mount(patch_item)
        .mount(delete_item);

    let merged_extra = App::new()
        .mount(html_page)
        .mount(json_page)
        .head("/head-check", || async {
            ResponseBuilder::new(StatusCode::OK)
                .header("x-head", "yes")
                .header("content-length", "42")
                .empty()
        });

    let app = App::new()
        .mount(hello)
        .mount(echo_post)
        .mount(inspect_req)
        .mount(missing)
        .nest("/api", nested_api)
        .merge(merged_extra);

    let server = TestServer::start(app);
    let base = server.base_url();

    let client = Client::builder()
        .user_agent("akku-test-agent/1.0")
        .default_header("X-Default", "enabled")
        .timeout(Duration::from_secs(5))
        .build();

    // GET route with path param.
    let res = client.get(format!("{base}/hello/world")).send().unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.is_success());
    assert_eq!(res.text().unwrap(), "hello world");

    // POST route with JSON body.
    let res = client
        .post(format!("{base}/echo"))
        .json_raw(r#"{"msg":"hi"}"#)
        .send()
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(res.header("x-seen-ct"), Some("application/json"));
    assert_eq!(res.text().unwrap(), r#"POST:{"msg":"hi"}"#);

    // Nested PUT / PATCH / DELETE routes.
    let res = client
        .put(format!("{base}/api/items/7"))
        .text("alpha")
        .send()
        .unwrap();
    assert_eq!(res.text().unwrap(), "PUT:7:alpha");

    let res = client
        .patch(format!("{base}/api/items/7"))
        .text("beta")
        .send()
        .unwrap();
    assert_eq!(res.text().unwrap(), "PATCH:7:beta");

    let res = client.delete(format!("{base}/api/items/7")).send().unwrap();
    assert_eq!(res.text().unwrap(), "DELETE:7");

    // Query encoding + default headers + multi-value request & response headers.
    let res = client
        .get(format!("{base}/inspect"))
        .query("q", "hello world")
        .query("a&b", "1+2")
        .header("X-Tag", "red")
        .header("X-Tag", "blue")
        .send()
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.text().unwrap(),
        "query=q=hello%20world&a%26b=1%2B2|ua=akku-test-agent/1.0|default=enabled|tags=red,blue"
    );
    assert_eq!(
        res.headers().get_all("x-resp-multi"),
        vec!["first", "second"]
    );

    // Merged HTML / JSON helpers.
    let html_res = client.get(format!("{base}/html")).send().unwrap();
    assert_eq!(
        html_res.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(html_res.text().unwrap(), "<p>hi</p>");

    let json_res = client.get(format!("{base}/json")).send().unwrap();
    assert_eq!(json_res.header("content-type"), Some("application/json"));
    assert_eq!(json_res.text().unwrap(), r#"{"ok":true}"#);

    // HEAD request must not hang waiting for Content-Length bytes.
    let head_res = client.head(format!("{base}/head-check")).send().unwrap();
    assert_eq!(head_res.status(), StatusCode::OK);
    assert_eq!(head_res.header("x-head"), Some("yes"));
    assert!(head_res.bytes().is_empty());

    // 404 + ensure_success.
    let err = client
        .get(format!("{base}/missing"))
        .send()
        .unwrap()
        .ensure_success()
        .unwrap_err();
    match err {
        ClientError::HttpStatus { status, body } => {
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(body, "no such item");
        }
        other => panic!("expected HttpStatus, got {other:?}"),
    }
}

#[test]
fn client_redirects_302_307_relative_and_limit() {
    let app = App::new()
        .mount(echo_post)
        .mount(redir_302)
        .mount(redir_307)
        .mount(redir_relative)
        .mount(redir_target)
        .mount(loop_a)
        .mount(loop_b);

    let server = TestServer::start(app);
    let base = server.base_url();
    let client = Client::new();

    // 302 converts POST -> GET and lands on /redir-target.
    let r302 = client
        .post(format!("{base}/redir-302"))
        .text("ignored-after-302")
        .send()
        .unwrap();
    assert_eq!(r302.status(), StatusCode::OK);
    assert_eq!(r302.text().unwrap(), "landed-get");

    // 307 preserves POST method and request body.
    let r307 = client
        .post(format!("{base}/redir-307"))
        .text("kept-body")
        .send()
        .unwrap();
    assert_eq!(r307.status(), StatusCode::CREATED);
    assert_eq!(r307.text().unwrap(), "POST:kept-body");

    // Relative redirect (`../redir-target` from `/nested/start`).
    let rel = client.get(format!("{base}/nested/start")).send().unwrap();
    assert_eq!(rel.status(), StatusCode::OK);
    assert_eq!(rel.text().unwrap(), "landed-get");

    // Redirect loop hits max_redirects limit.
    let strict = Client::builder().max_redirects(2).build();
    let err = strict.get(format!("{base}/loop-a")).send().unwrap_err();
    match err {
        ClientError::TooManyRedirects(limit) => assert_eq!(limit, 2),
        other => panic!("expected TooManyRedirects(2), got {other:?}"),
    }
}

#[test]
fn client_chunked_eof_https_and_timeout() {
    // 1. Chunked transfer encoding (including chunk extension and trailer) via raw TcpListener.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut req_buf = [0u8; 1024];
        let _ = sock.read(&mut req_buf);
        let raw_resp = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Transfer-Encoding: chunked\r\n",
            "X-Stream: chunked\r\n",
            "\r\n",
            "5;ext=1\r\n",
            "hello\r\n",
            "7\r\n",
            " world!\r\n",
            "0\r\n",
            "X-Trailer: done\r\n",
            "\r\n"
        );
        sock.write_all(raw_resp.as_bytes()).unwrap();
    });

    let client = Client::new();
    let res = client.get(format!("http://{addr}/chunked")).send().unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.header("x-stream"), Some("chunked"));
    assert_eq!(res.text().unwrap(), "hello world!");
    handle.join().unwrap();

    // 2. EOF-delimited HTTP/1.0 response (no Content-Length, no Transfer-Encoding).
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut req_buf = [0u8; 1024];
        let _ = sock.read(&mut req_buf);
        let raw_resp = "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\neof-body";
        sock.write_all(raw_resp.as_bytes()).unwrap();
    });

    let res = client.get(format!("http://{addr}/eof")).send().unwrap();
    assert_eq!(res.text().unwrap(), "eof-body");
    handle.join().unwrap();

    // 3. https:// produces a clear UnsupportedScheme error explaining Wave 2 TLS.
    let err = client.get("https://example.com/test").send().unwrap_err();
    match &err {
        ClientError::UnsupportedScheme(scheme) => assert_eq!(scheme, "https"),
        other => panic!("expected UnsupportedScheme(https), got {other:?}"),
    }
    let msg = err.to_string();
    assert!(msg.contains("https") && msg.contains("Wave 2"), "msg={msg}");

    // 4. Socket read timeout maps to ClientError::TimedOut.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (sock, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(250));
        drop(sock);
    });

    let fast = Client::builder().timeout(Duration::from_millis(40)).build();
    let err = fast.get(format!("http://{addr}/slow")).send().unwrap_err();
    assert!(
        matches!(err, ClientError::TimedOut),
        "expected TimedOut, got {err:?}"
    );
    handle.join().unwrap();
}
