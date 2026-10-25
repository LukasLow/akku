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
