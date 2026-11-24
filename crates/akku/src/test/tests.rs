use std::fs;

use akku::test::{Snapshot, TempDir};

#[test]
fn temp_dir_lifecycle_write_read_persist_and_close() {
    let path_after_drop = {
        let dir = TempDir::with_prefix("akku-unit").unwrap();
        assert!(dir.path().exists());
        let nested = dir.write("a/b/file.txt", "hello world").unwrap();
        assert!(nested.exists());
        assert_eq!(dir.read_to_string("a/b/file.txt").unwrap(), "hello world");
        dir.path().to_path_buf()
    };
    assert!(!path_after_drop.exists(), "TempDir should clean up on Drop");

    // Persist preserves directory after ownership ends.
    let persisted = TempDir::new().unwrap().persist();
    assert!(persisted.exists());
    fs::remove_dir_all(&persisted).unwrap();

    // Explicit close removes directory immediately.
    let dir2 = TempDir::new().unwrap();
    let p2 = dir2.path().to_path_buf();
    dir2.close().unwrap();
    assert!(!p2.exists());
}

#[test]
fn snapshot_update_compare_crlf_and_lcs_diff() {
    let dir = TempDir::new().unwrap();
    let snap_file = dir.child("sample.snap");

    // Missing snapshot fails cleanly.
    let missing = Snapshot::at(&snap_file).update(false).compare("hello\n");
    assert!(missing.is_err());

    // Update mode writes snapshot.
    Snapshot::at(&snap_file)
        .update(true)
        .assert_eq("line1\nline2\nline3\n");

    // Exact match passes, including CRLF normalization.
    Snapshot::at(&snap_file)
        .update(false)
        .assert_eq("line1\r\nline2\r\nline3\r\n");

    // Mismatch produces LCS line diff.
    let diff = Snapshot::at(&snap_file)
        .update(false)
        .compare("line1\nchanged\nline3\nadded\n")
        .unwrap_err();
    let msg = diff.to_string();
    assert!(msg.contains("- line2"), "diff missing removal:\n{msg}");
    assert!(msg.contains("+ changed"), "diff missing insertion:\n{msg}");
    assert!(
        msg.contains("+ added"),
        "diff missing trailing insertion:\n{msg}"
    );

    // Large fallback diff (>2000 lines) works quickly without huge DP allocation.
    let big_expected: String = (0..2100).map(|i| format!("row-{i}\n")).collect();
    let big_actual = big_expected.replace("row-1500", "row-1500-edited");
    let big_snap = dir.child("big.snap");
    Snapshot::at(&big_snap)
        .update(true)
        .assert_eq(&big_expected);
    let big_diff = Snapshot::at(&big_snap)
        .update(false)
        .compare(&big_actual)
        .unwrap_err();
    assert!(big_diff.to_string().contains("row-1500-edited"));
}

#[cfg(feature = "http")]
#[test]
fn http_test_server_and_client_round_trip() {
    use akku::html::Html;
    use akku::http::{App, Path};
    use akku::test::{TestClient, TestServer};

    #[akku::http::get("/hello/{name}")]
    async fn hello(Path(name): Path<String>) -> Html {
        Html::new(format!("<h1>Hello {name}</h1>"))
    }

    #[akku::http::post("/echo")]
    async fn echo(body: String) -> String {
        format!("echo:{body}")
    }

    // Via TestServer + client()
    let app = App::new().mount(hello).mount(echo);
    let server = TestServer::start(app);
    assert!(server.base_url().starts_with("http://127.0.0.1:"));

    let client = server.client();
    let res = client.get("/hello/akku").send().unwrap();
    res.assert_ok()
        .assert_status(200)
        .assert_header("content-type", "text/html; charset=utf-8")
        .assert_body_eq("<h1>Hello akku</h1>");

    let post_res = client.post("/echo").text("ping").send().unwrap();
    post_res
        .assert_status(200)
        .assert_body_contains("echo:ping");

    let missing = client.get("/does-not-exist").send().unwrap();
    missing.assert_status(404);

    drop(server);

    // Via standalone TestClient::new(app) owning its own server.
    let client2 = TestClient::new(App::new().mount(hello));
    client2
        .get("/hello/world")
        .send()
        .unwrap()
        .assert_ok()
        .assert_body_contains("Hello world");
}
