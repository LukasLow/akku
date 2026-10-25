# Akku — Konzeptpapier

Stand: 2026-10-25  
Repo/Workspace: `akku`  
Produkt/Crate: `akku`

## 1. Vision

Akku ist ein **Batterieset für Rust**, kein Ersatz für die Rust-Stdlib.

Ein neues Projekt soll mit genau einer Abhängigkeit beginnen können:

```toml
[dependencies]
akku = { version = "0.1", features = ["http", "json"] }
```

Zielgefühl:

> Ich starte ein Projekt, meine Abhängigkeiten sind `akku` und `std` — und alles, was ich für einen normalen Dienst brauche, ist da, konsistent und angenehm.

Akku soll **nicht** an beiden Enden des Spektrums leben:

- **nicht** „100 % null externe Dependencies“ als Dogma,
- **nicht** „ein Mega-Crate, das nur fremde APIs durchreicht“.

Sondern: eine kuratierte, stabile, gut benennbare Oberfläche über bewusst ausgewählten Implementierungen. Weniger Dependencies sind besser, aber Sicherheit und Wartbarkeit schlagen Dependency-Purismus.

## 2. Nicht-Ziele

- Keine vollständige Rust-Stdlib ersetzen.
- Kein eigener TLS-, Crypto-, HTTP/2/3- oder QUIC-Stack im ersten Schritt.
- Keine 300 Domain-Crates, die später Namen blockieren.
- Keine reine 1:1-Weiterleitung von axum/serde/tokio.
- Keine Platzhalter-Crates auf crates.io, nur um Namen zu sichern.
- Kein Git-Subtree als primärer Distributionsweg.

## 3. Produktform

### Ein Crate nach außen

`akku` ist die einzige Dependency für Nutzer. Alle Domänen sind Module und Features:

```text
akku::core      Bytes, ASCII, Endian, Fehler
akku::http      Server, Router, Request/Response
akku::json      JSON-Werte, Serialisierung
akku::xml       XML
akku::time      Zeit und Dauer
akku::units     Einheiten
akku::math      Mathematik
akku::tls       TLS-Adapter
```

### Zwei Crates, die es wirklich braucht

```text
akku/                 # Repo/Workspace, kein Crate-Name
  crates/
    akku/                 # die eine Dependency
    akku-drive/           # proc-macro = true
    akku-cli/             # optional, später
```

- `akku-drive` ist unvermeidbar: Rust erlaubt Proc-Macros nur in einer eigenen
  `proc-macro = true`-Crate.
- `akku` re-exportiert die Makros, sodass Nutzer nur `akku` sehen.
- `akku-cli` ist optional und wird nicht Teil des v0-Ziels.

`akku` ist der Repo-/Workspace-Name, `akku` das Produkt. Damit gibt es nur
zwei frühe Crate-Namen statt 300.

## 4. Feature-Modell

Cargo hat kein hartes Feature-Limit. Praktisch sind **8–12 Domänen-Features**
sinnvoll; nicht ein Feature pro Funktion.

```toml
[features]
default = []                    # core + std, keine schweren Deps
http    = ["dep:axum", "dep:tokio"]
http-native = []                # eigener HTTP/1.1-Kern, später
json    = ["dep:serde", "dep:serde_json"]
xml     = ["dep:quick-xml"]
time    = ["dep:time"]
units   = []                    # eigener Code
math    = []                    # eigener Code
tls     = ["dep:rustls"]
full    = ["http", "json", "xml", "time", "units", "math", "tls"]
```

Regeln:

- `default = []`; schwere Domänen sind opt-in.
- `full` ist eine Bequemlichkeits-Meta-Feature, kein Default.
- `dep:`-Syntax verwenden, damit optionale Dependencies nicht als implizite
  Features erscheinen.
- Features sind additiv und werden im gesamten Dependency-Graph vereinigt.
- CI testet: `--no-default-features`, Default, jedes Einzelfeature, `full`.
  Nicht den vollständigen Powerset.
- `cargo tree -e features -p akku` ist die Kontrolle, nicht die Cargo.toml.

### Was landet im Binary?

Cargo baut jede aktivierte Crate im Dependency-Graph, auch wenn nur eine
Funktion daraus verwendet wird. Der Linker/LTO entfernt ungenutzten Code aus dem
fertigen Binary, aber die Compile-Zeit bleibt. Deshalb:

- `akku::json` ohne Feature `http` baut kein axum/tokio.
- `akku::http` baut axum/tokio nur, wenn das Feature aktiviert ist.
- Ein großer Feature-Default würde dieses Ziel zerstören.

## 5. Dependency-Politik

### Kern

`akku::core`, `akku::units`, `akku::math` und die kleinen Basismodule sollen
ohne externe Crates auskommen. Sie dürfen `core`, `alloc` und `std` nutzen.

### Adapter

Wo eine eigene Implementierung unverhältnismäßig oder sicherheitskritisch wäre,
wird bewusst delegiert:

| Domäne | Strategie |
|---|---|
| TLS | `rustls` (oder eine andere geprüfte Implementierung), nie selbst |
| Crypto | geprüfte Crates, nie selbst |
| HTTP/1.1 | zuerst axum, später optional eigener Kern |
| HTTP/2/3, QUIC | nicht im v0-Ziel |
| JSON | `serde_json` hinter einer kleinen API |
| XML | `quick-xml` hinter einer kleinen API |
| Zeit | `time` oder eigener kleiner Kern; Entscheidung offen |

### Versionen und Audit

- Direkte Dependencies werden **exakt gepinnt**: `axum = "=0.8.9"`, nicht
  `"0.8"`.
- `cargo deny`, `cargo audit`, `cargo vet` und `cargo tree` laufen in CI.
- Kleine Crates werden gelesen. Bei tokio/hyper/axum ist ein kompletter
  Zeilen-Audit unrealistisch; stattdessen: etablierte Crates, exakte Pins,
  Advisory-Checks und eigene Wrapper klein halten.
- Sicherheitskritische Kerne werden nicht aus Prestige selbst gebaut.

## 6. API-Prinzipien

1. **User-first.** Erst den Code schreiben, den man als Nutzer haben will; dann
   die Implementierung.
2. **Ein offensichtlicher Weg.** Kein Trait-Labyrinth für den Normalfall.
3. **Escape-Hatch.** Wer die volle axum-/serde-API braucht, bekommt sie über
   `akku::http::raw()` bzw. `akku::json::raw()`.
4. **Makros sind Zucker, keine Voraussetzung.** Die manuelle API muss ohne
   Makros angenehm sein.
5. **Fehler vereinheitlichen.** Ein `akku::Result<T>` und ein
   `akku::Error`, das Kontext und Quelle erhält.
6. **Defaults sicher und performant.** Kein optionales Sicherheitsfeature.
7. **Namen stabil halten.** Feature-Namen und öffentliche Typen sind API.

## 7. HTTP: axum wrappen oder selbst bauen?

### Variante A — `akku::http` als axum-Fassade (v0)

Kurzfristig die pragmatische Wahl:

- eigene, kleine API,
- axum/tokio als Implementierung hinter `feature = "http"`,
- Escape-Hatch auf den echten axum-Router.

Vorteile: sofort produktiv, ausgereifte HTTP-Implementierung, weniger Risiko.  
Nachteile: axum/tokio bleiben im Graph, die API ist von axum geprägt.

### Variante B — eigener HTTP/1.1-Kern

„Axum komplett selbst bauen“ heißt technisch:

- eigener Request-Parser (Request-Line, Header, Body, Chunked),
- eigener Router,
- eigene Response- und Body-Typen,
- Keep-Alive, Timeouts, Limits, Backpressure,
- TLS über `rustls`.

Das ist für HTTP/1.1 machbar, aber sicherheitskritisch. Request-Smuggling,
Header-Limits, langsame Clients und Body-Limits müssen von Anfang an korrekt
sein. Ein eigener HTTP/2/3-Stack ist ein eigenes Mehrjahresprojekt und gehört
nicht in v0.

Wenn der eigene Kern kommt, sollte er **hinter derselben API** wie Variante A
liegen. Nutzer dürfen den Unterschied nicht in ihrem Code merken.

### Variante C — eigener Router, aber hyper/tokio darunter

Mittelweg: eigener Router, eigene Extractor-/Response-Schicht, aber hyper
bleibt der HTTP-Parser. Das reduziert das Rad, das neu erfunden wird, und
behält die API-Kontrolle. Diese Variante ist wahrscheinlich der beste
Kompromiss, falls axum sich zu schwer anfühlt.

### Empfehlung

1. **API zuerst definieren**, unabhängig von axum.
2. v0: axum-Adapter hinter `http`.
3. Danach: eigener HTTP/1.1-Kern als experimentelles Feature
   `http-native`, gleiche API.
4. hyper/tokio nur ersetzen, wenn Messungen und Wartbarkeit es rechtfertigen.
5. HTTP/2/3/TLS nicht selbst bauen.

## 8. API-Skizzen

### Ohne Makro: Builder + Handler

```rust
use akku::http::{App, Request, Response, Result};

#[akku::main]
async fn main() -> Result<()> {
    App::new()
        .get("/", home)
        .get("/users/{id}", user)
        .serve("127.0.0.1:3000")
        .await
}

async fn home(_: &Request) -> Response {
    Response::html("<h1>Hello</h1>")
}

async fn user(request: &Request) -> Result<Response> {
    let id: u64 = request.param("id")?.parse()?;
    Ok(Response::html(format!("<h1>User {id}</h1>")))
}
```

Das ist bereits eine vollständige API. Kein Makro nötig.

### Mit typisierten Extractors

```rust
use akku::http::{Html, Path};

async fn user(Path(id): Path<u64>) -> Html<String> {
    Html(format!("<h1>User {id}</h1>"))
}
```

Der Router kann trotzdem ohne Makro registriert werden:

```rust
App::new().get("/users/{id}", user)
```

### Mit Makro als Zucker

```rust
let app = akku::http::router! {
    GET "/" => home,
    GET "/users/{id}" => user,
};
```

oder:

```rust
#[akku::get("/users/{id}")]
async fn user(Path(id): Path<u64>) -> Html<String> {
    Html(format!("<h1>User {id}</h1>"))
}
```

### Braucht man dann noch Makros?

**Nein, nicht für die Grundfunktion.** Ein Builder plus typisierte Extractors
reicht. Makros bleiben sinnvoll für:

- große Route-Tabellen und Compile-Zeit-Prüfung von Methoden/Pfaden,
- `#[derive(Form)]`, `#[derive(ApiError)]`, `#[derive(Model)]`,
- wiederkehrende Handler-Signaturen und Body-/Query-Extraktion,
- eine einheitliche `#[akku::main]`-Hülle.

Die Regel lautet: **erst die manuelle API angenehm machen, dann Makros
hinzufügen**. Ein Makro über einer schlechten API macht sie nicht gut, sondern
nur schwerer zu debuggen.

## 9. Beispiel: Original vs. Akku

### Original mit axum

```rust
use axum::{extract::Path, response::Html, routing::get, Router};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route("/", get(home))
        .route("/users/{id}", get(user));

    let listener = TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn home() -> Html<&'static str> {
    Html("<h1>Hello</h1>")
}

async fn user(Path(id): Path<u64>) -> Html<String> {
    Html(format!("<h1>User {id}</h1>"))
}
```

### Akku, ohne Router-Makro

```rust
use akku::http::{App, Html, Path, Result};

#[akku::main]
async fn main() -> Result<()> {
    App::new()
        .get("/", home)
        .get("/users/{id}", user)
        .serve("127.0.0.1:3000")
        .await
}

async fn home() -> Html<&'static str> {
    Html("<h1>Hello</h1>")
}

async fn user(Path(id): Path<u64>) -> Html<String> {
    Html(format!("<h1>User {id}</h1>"))
}
```

### Akku, mit Router-Makro

```rust
use akku::http::{Html, Path, Result};

#[akku::main]
async fn main() -> Result<()> {
    akku::http::serve(akku::http::router! {
        GET "/" => home,
        GET "/users/{id}" => user,
    })
    .bind("127.0.0.1:3000")
    .await
}

async fn home() -> Html<&'static str> {
    Html("<h1>Hello</h1>")
}

async fn user(Path(id): Path<u64>) -> Html<String> {
    Html(format!("<h1>User {id}</h1>"))
}
```

Der Unterschied ist ehrlich: Für zwei Routen ist das Makro kein großer Gewinn.
Es gewinnt bei vielen Routen, Compile-Zeit-Prüfung und einheitlichen Fehlern.
Wenn es das nicht leistet, bleibt es optional.

## 10. Namens- und Distributionsentscheidung

- `akku` und `akku-drive` sind auf crates.io aktuell frei.
- `akku` ist als Repo-/Workspace-Name frei.
- Kein Git-Subtree als primärer Weg: Subtree kopiert Quellcode und hat keine
  Semver-Auflösung.
- Während der Entwicklung: `path`-Dependency im Workspace.
- Für frühe Nutzer: `git`-Dependency mit festem `rev`.
- Sobald die API steht: `akku` und `akku-drive` mit echtem Minimalcode
  veröffentlichen, um die Namen zu sichern.
- Keine 300 Platzhalter-Crates.

## 11. Roadmap

### Phase 0 — Konzept und API-Dream

- [ ] `docs/API-DREAM.md`: nur Nutzercode, keine Interna.
- [ ] 5 Kernbeispiele: Hello HTTP, JSON-API, Formular, Zeit/Units, Makro.
- [ ] Entscheidung: `default = []` oder ein kleines Default-Set.
- [ ] Entscheidung: eigener HTTP-Kern als `http-native` oder nur axum.

### Phase 1 — Kern ohne schwere Dependencies

- [ ] `akku` + `akku-drive` Workspace.
- [ ] `core`: Error, Bytes, ASCII, Endian.
- [ ] `units` und `math` als eigene, kleine Module.
- [ ] Tests, Docs, CI, `cargo deny`/`audit`.

### Phase 2 — Erste echte Domänen

- [ ] `json` hinter `serde_json`, kleine stabile API.
- [ ] `http` hinter axum, Builder + typisierte Extractors.
- [ ] Drei Beispiele: Hello, JSON, Form.
- [ ] `cargo tree`-Budget und Feature-Matrix in CI.

### Phase 3 — Eigener HTTP-Kern (Spike)

- [ ] HTTP/1.1-Parser und Router hinter derselben API.
- [ ] Sicherheitsfälle: Limits, Timeouts, Chunked, Smuggling.
- [ ] Benchmark gegen axum; nur behalten, wenn es einen klaren Vorteil gibt.

### Phase 4 — Makros und mehr Domänen

- [ ] `router!`, `#[akku::main]`, `#[derive(Form)]`.
- [ ] `time`, `xml`, `tls` nach Bedarf.
- [ ] `akku-cli` nur, wenn ein echter Nutzen entsteht.

## 12. Erfolgskriterien

Akku v0 ist erfolgreich, wenn:

- ein neues Projekt nur `akku` + `std` in der Cargo.toml braucht,
- drei Beispiele ohne versteckte Crate-Imports kompilieren,
- `cargo tree -e features` nur die aktivierten Domänen zeigt,
- die manuelle API ohne Makros angenehm ist,
- die Makros die API messbar kürzer/klarer machen,
- `cargo deny`/`audit` grün sind,
- die HTTP-Performance mindestens auf Augenhöhe mit direktem axum ist,
- ein Nutzer ohne Akku leicht zu axum/serde zurückkehren kann.

## 13. Offene Entscheidungen

1. **HTTP-Kern:** v0 nur axum, oder sofort eigener HTTP/1.1-Kern?
2. **Default-Features:** leer, oder `http` + `json` als kleines Default?
3. **Async-Runtime:** tokio akzeptieren, oder langfristig eigener Kern?
4. **TLS:** `rustls` direkt, oder eigener `akku::tls`-Adapter mit mehreren Backends?
5. **API-Stil:** Builder zuerst, `router!` später — oder Makro zuerst?
6. **Name:** `akku` Repo, `akku` Crate; `akku-drive` für Makros.
7. **Distribution:** crates.io ab v0.1, git-dependency vorher.

## 14. Leitentscheidung

**Akku wird zuerst eine gute API mit einer kuratierten Implementierung.**
Nicht zuerst ein Dependency-Sammler. Nicht zuerst ein Mega-Crate. Nicht zuerst
eine Stdlib.

Die Reihenfolge ist:

1. Nutzercode träumen.
2. API ohne Makros angenehm machen.
3. Implementierung hinter Features verstecken.
4. Makros nur für echte Wiederholung.
5. Dependencies exakt pinnen und auditieren.
6. Erst dann über eigene HTTP-/Runtime-Kerne reden.
