use akku::html::Html;
use akku::http::{App, Path, Result};

#[akku::http::get("/")]
async fn home() -> Html {
    Html::new("<h1>Hello from Akku</h1>")
}

#[akku::http::get("/users/{id}")]
async fn user(Path(id): Path<u64>) -> Html {
    Html::new(format!("<h1>User {id}</h1>"))
}

#[akku::http::main]
async fn main() -> Result<()> {
    App::new()
        .mount(home)
        .mount(user)
        .serve("127.0.0.1:3000")
        .await
}
