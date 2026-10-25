# Akku Namespace

Dieses Dokument ist die **Namensverfassung** von Akku. Es verhindert, dass
spätere Domänen einander überschreiben. Es ist **keine** Kopie der 320
MojoAkku-Katalogdateien; die Katalogliste bleibt in MojoAkku. Hier stehen die
Regeln und die kanonischen Pfade.

## Grundregel

Jeder öffentliche Name lebt unter einer Domäne:

```rust
akku::http::get
akku::http::server
akku::html::Html
akku::json::from_str
akku::time::Instant
akku::units::Meter
```

Nicht:

```rust
akku::get
akku::Html
akku::from_str
```

Der Grund: `get`, `server`, `Value`, `Error` und `Instant` sind generische
Namen. Zwei Domänen werden sie früher oder später beide brauchen. Der
Domänenpfad ist die Kollisionsgrenze, nicht ein globaler Präfix-Zoo.

## Domänen statt Crate-Namen

Die Domänen sind **Module in `akku`**, keine eigenen Crates. Damit gibt es
keine 300 Crate-Namen, die von anderen belegt werden können.

```text
akku/
  src/
    lib.rs
    http/
    html/
    json/
    xml/
    time/
    units/
    math/
    net/
    ...
```

Ein eigener Crate-Name entsteht nur, wenn es technisch nicht anders geht:

- `akku-drive` — Proc-Macros (`proc-macro = true`)
- `akku-cli` — optionales Binary

## Networking

| Konzept | Kanonischer Pfad | Typ/Regel |
|---|---|---|
| HTTP-Server | `akku::http::server` | `HttpServer`; nie global `Server` |
| HTTP-Client | `akku::http::client` | `HttpClient`; nie global `Client` |
| HTTP GET | `akku::http::get` | Attribut-Makro |
| HTTP POST | `akku::http::post` | Attribut-Makro |
| HTTP Request | `akku::http::Request` | domänenlokal |
| HTTP Response | `akku::http::Response` | domänenlokal |
| HTTP Status | `akku::http::Status` | domänenlokal |
| HTTP Header | `akku::http::Headers` | Plural, weil es eine Map ist |
| HTTP Cookie | `akku::http::Cookie` | domänenlokal |
| HTTP Route | `akku::http::Route` | domänenlokal |
| HTTP Fehler | `akku::http::Error` | domänenlokal |
| TCP | `akku::net::tcp` | `TcpStream`, `TcpListener` |
| UDP | `akku::net::udp` | `UdpSocket` |
| Socket | `akku::net::socket` | `Socket`, `SocketAddr` |
| IP | `akku::net::ip` | `IpAddr`, `Ipv4Addr`, `Ipv6Addr` |
| DNS | `akku::net::dns` | eigener Namensraum |
| TLS | `akku::tls` | `TlsConfig`, `TlsStream`; Backend intern |
| WebSocket | `akku::websocket` | eigener Namensraum, nicht `http::websocket` |
| SSE | `akku::sse` | eigener Namensraum |
| Proxy | `akku::proxy` | eigener Namensraum |

### Kollisionsregel

Wenn zwei Konzepte denselben Kurznamen brauchen, wird **nicht** die Domäne
verdoppelt, sondern das Konzept spezifiziert:

```rust
akku::http::server   // HTTP-Server
akku::http::client   // HTTP-Client

akku::net::tcp       // TCP
akku::net::udp       // UDP
```

Nicht:

```rust
akku::http_server    // flach, kollidiert mit einem möglichen Server-Modul
akku::net_tcp        // Präfix statt Modul
```

Nur wenn ein Name auch innerhalb der Domäne mehrdeutig ist, kommt ein Suffix:

```rust
akku::http::server::native
akku::http::server::axum
akku::http::client::blocking
akku::http::client::async
```

## Web und Markup

| Konzept | Kanonischer Pfad | Hinweis |
|---|---|---|
| HTML | `akku::html` | eigener Namensraum, getrennt von HTTP |
| HTML-Escaping | `akku::html::escape` | reine Funktion |
| HTML-Dokument | `akku::html::Document` | domänenlokal |
| HTML-Element | `akku::html::Element` | domänenlokal |
| URL | `akku::url` | eigener Namensraum |
| URI | `akku::uri` | eigener Namensraum, falls nötig |
| MIME | `akku::mime` | eigener Namensraum |
| Cookie | `akku::http::Cookie` | HTTP-nah; nicht global |
| Formular | `akku::form` | eigener Namensraum |
| Template | `akku::template` | eigener Namensraum |
| Markdown | `akku::markdown` | eigener Namensraum |

**Wichtig:** `akku::html` ist kein Untermodul von `akku::http`. HTML ist eine
eigene Domäne; HTTP kann `Html` als Response-Body verwenden, aber die
Abhängigkeit zeigt von HTTP nach HTML, nicht umgekehrt.

## Daten, Formate, Codierung

| Domäne | Kanonischer Pfad | Beispiele |
|---|---|---|
| JSON | `akku::json` | `Value`, `from_str`, `to_string`, `json!` |
| XML | `akku::xml` | `Element`, `parse`, `to_string` |
| YAML | `akku::yaml` | eigener Namensraum |
| TOML | `akku::toml` | eigener Namensraum |
| CSV | `akku::csv` | eigener Namensraum |
| Base64 | `akku::codec::base64` | eigener Codec-Namensraum |
| Hex | `akku::codec::hex` | eigener Codec-Namensraum |
| Kompression | `akku::compress` | `gzip`, `zstd`, `zip` als Untermodule |
| Serialisierung | `akku::serde` | nur wenn es eine eigene Fassade ist |

## Mathematik, Zeit, Einheiten

| Domäne | Kanonischer Pfad | Beispiele |
|---|---|---|
| Mathematik | `akku::math` | `sqrt`, `sin`, `Vec2`, `Matrix` |
| Statistik | `akku::stat` | eigener Namensraum |
| Algorithmen | `akku::algo` | `sort`, `search`, `graph` |
| Einheiten | `akku::units` | `Meter`, `Second`, `Kilogram` |
| Zeit | `akku::time` | `Instant`, `Duration`, `Date` |
| Datum/Uhrzeit | `akku::time::datetime` | Untermodul, kein globales `DateTime` |

## System und Infrastruktur

| Domäne | Kanonischer Pfad | Beispiele |
|---|---|---|
| Dateisystem | `akku::fs` | `File`, `Path`, `read`, `write` |
| I/O | `akku::io` | `Read`, `Write`, `Cursor` |
| Betriebssystem | `akku::os` | `env`, `process`, `signal` |
| CLI | `akku::cli` | `Args`, `Command`, `Terminal` |
| Synchronisation | `akku::sync` | `Mutex`, `Channel`, `Atomic` |
| Async | `akku::async` | `spawn`, `Runtime`, `Task` |
| Distribution | `akku::dist` | `Actor`, `Broker`, `Cluster` |
| Build | `akku::build` | `Package`, `Target`, `Version` |
| Logging | `akku::log` | `info!`, `warn!`, `Logger` |
| Testing | `akku::test` | `assert_json`, `Mock` |
| Benchmark | `akku::bench` | `black_box`, `measure` |

## Krypto, Security, Datenbanken

| Domäne | Kanonischer Pfad | Regel |
|---|---|---|
| Crypto | `akku::crypto` | nur geprüfte Backends; nie selbst |
| Hash | `akku::crypto::hash` | Untermodul |
| Signatur | `akku::crypto::sign` | Untermodul |
| Zufall | `akku::crypto::rng` | Untermodul |
| Security | `akku::security` | Auth, ACL, JWT, OAuth |
| TLS | `akku::tls` | eigener Namensraum, Backend intern |
| Datenbank | `akku::db` | `sql`, `kv`, `cache` als Untermodule |
| ORM | `akku::db::orm` | Untermodul |
| Migration | `akku::db::migrate` | Untermodul |

## Sprache, Meta, Collections

| Domäne | Kanonischer Pfad | Beispiele |
|---|---|---|
| Collections | `akku::coll` | `Map`, `Set`, `List`, `Iter` |
| Sprache | `akku::lang` | `Result`, `Option`, `Drop`-Helfer |
| Meta | `akku::meta` | `TypeId`, `Reflect`, `Macro` |
| Memory | `akku::mem` | `Arena`, `Pool`, `Pin` |
| Primitive | `akku::prim` | `bit`, `endian`, `limits` |
| Development | `akku::dev` | `debug`, `profile`, `mock` |

## Medien, UI, Anwendungen

| Domäne | Kanonischer Pfad | Beispiele |
|---|---|---|
| Media | `akku::media` | `image`, `audio`, `video`, `color` |
| UI | `akku::ui` | `Component`, `Theme`, `Chart` |
| App | `akku::app` | `Tauri`-artige Anwendungen, `window`, `webview` |
| Dokumente | `akku::document` | `pdf`, `docx`, `epub` |

## Sonderfälle

Diese Namen sind **reserviert** und dürfen nicht als globale Kurznamen
auftauchen:

```text
Server, Client, Request, Response, Error, Result, Value, Map, Set,
List, Node, Element, Document, Route, Handler, Context, Config
```

Sie leben immer unter einer Domäne:

```rust
akku::http::Request
akku::html::Element
akku::json::Value
akku::coll::Map
akku::http::Error
akku::http::Result
```

## Makros

Makros folgen derselben Domänenregel:

```rust
#[akku::http::get("/users/{id}")]
#[akku::http::post("/users")]
#[akku::http::main]
#[akku::html::render]        // später, falls nötig
#[akku::json::model]         // später, falls nötig
```

Nicht:

```rust
#[akku::get]
#[akku::main]
```

`akku-drive` ist die Implementierung; Nutzer importieren nur `akku`. Das
Makro darf intern `::akku::http::__private` verwenden, aber die öffentliche
Doku zeigt immer den Domänenpfad.

## Import-Regel

Ein `prelude` darf Bequemlichkeit anbieten, aber keine Namenshoheit
übernehmen:

```rust
use akku::prelude::*;
```

Das Prelude re-exportiert nur eine kleine, kuratierte Menge. Wer einen Namen
braucht, der nicht im Prelude ist, importiert ihn aus der Domäne. Damit bleibt
der vollständige Pfad immer eindeutig.

## MojoAkku-Mapping

Die MojoAkku-Domänen werden nicht 1:1 als 320 Rust-Module übernommen. Die
Kategorien bleiben, aber die Zahl der öffentlichen Domänen bleibt klein:

| MojoAkku-Bereich | Akku-Domäne |
|---|---|
| `net_`, `proto_`, `web_` | `akku::http`, `akku::net`, `akku::tls`, `akku::websocket` |
| `text_`, `markup_`, `format_` | `akku::html`, `akku::json`, `akku::xml`, `akku::template` |
| `math_`, `stat_`, `algo_`, `units_` | `akku::math`, `akku::stat`, `akku::algo`, `akku::units` |
| `crypto_`, `security_` | `akku::crypto`, `akku::security`, `akku::tls` |
| `os_`, `fs_`, `io_`, `cli_`, `sync_`, `async_` | `akku::os`, `akku::fs`, `akku::io`, `akku::cli`, `akku::sync`, `akku::async` |
| `coll_`, `lang_`, `meta_`, `mem_`, `prim_` | `akku::coll`, `akku::lang`, `akku::meta`, `akku::mem`, `akku::prim` |
| `media_`, `ui_`, `app_` | `akku::media`, `akku::ui`, `akku::app` |

Die konkrete API entsteht user-first in `docs/API.md`. Dieses Dokument
entscheidet nur, **wo** ein Name lebt, nicht **wie** er funktioniert.
