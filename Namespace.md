# Akku Namespace

Dieses Dokument ist die **Namensverfassung** von Akku. Es regelt, wie öffentliche
Pfade heißen, wann Familien entstehen und welche Namenskollisionen verboten sind.

## Grundregeln

1. **Eine Library-Crate:** Alle öffentlichen Module leben unter `akku::*`. Es gibt
   keine 20 oder 300 Einzel-Crates — nur `akku`, das technisch erzwungene
   Proc-Macro-Crate `akku-drive` (von `akku` re-exportiert) und das optionale
   Binary `akku-cli`.
2. **Kein globales Symbol-Sammelbecken:** Es gibt kein `akku::get`,
   `akku::Request`, `akku::Server` oder `akku::Value`. Jeder öffentliche Typ und
   jedes Makro lebt in seinem Modul (`akku::http::get`, `akku::json::Value`).
3. **Alles flach als Default:** Jede eigenständige Library startet direkt auf
   Ebene 1 (`akku::<name>`). Keine künstliche Taxonomie wie `akku::web::http`
   oder `akku::encoding::json`.
4. **Dynamische Familien:** Wenn mehrere eng zusammengehörige Module organisch
   wachsen, dürfen sie in eine Familie gebündelt werden:
   - vor `1.0` direkt,
   - ab `1.0` über `#[deprecated] pub use familie::modul;` Übergangs-Aliase bis
     zur nächsten Major-Version.
5. **Kurze, prägnante Namen:** Kurze kanonische Namen werden bevorzugt (`ws`,
   `sse`, `pic`, `vid`, `phy`, `cli`, `fs`, `kv`, `db`-Einzelmodule `sql`/`kv`,
   `url`, `tls`).
6. **Keine Rust-Keywords und kein Shadowing:** Reservierte Schlüsselwörter sind
   als Modulnamen verboten (`async`, `gen`, `try`, `box`, `mod`, `type`, `move`,
   `ref`, `impl`). Statt `akku::async` heißt das Modul `akku::task`. `akku::core`
   wird vermieden, um `::core` nicht zu überschatten.
7. **Discovery über Tags, nicht über tiefe Bäume:** Querschnitts-Zuordnungen
   (`#web`, `#cli`, `#wasm32`, `#native-0dep`, `#adapter`, `#fork`, `#maintainer`)
   werden in Modul-Metadaten (`meta.toml`) gepflegt und im Katalog (z. B. GitHub
   Pages) sortiert.

---

## Familien (gruppierte Module)

Die folgenden Bereiche sind bewusst als Familien gruppiert (Ebene 1 = Familie,
Ebene 2 = Einzelmodule):

| Familie | Inhalt / Beispiele | Grund |
|---|---|---|
| `akku::archive` | `gzip`, `zstd`, `zip`, `tar` | Gemeinsame Stream-/Archiv-Familie |
| `akku::auth` | `password`, `jwt`, `oauth2`, `totp`, `webauthn`, `session`, `acl` | Authentifizierung, Identität & Zugriffskontrolle |
| `akku::audio` | `pcm`, `wav`, `flac`, `dsp`, `midi`, `device` | Getrennt von Bild/Video (z. B. für DAWs) |
| `akku::vid` | `frame`, `codec`, `mux`, `stream` | Getrennt von Audio/Bild |
| `akku::pic` | `png`, `jpeg`, `webp`, `bmp`, `canvas`, `filter` | Getrennt von Audio/Video (z. B. Bildeditoren) |
| `akku::net` | `ip`, `socket`, `tcp`, `udp`, `uds`, `dns`, `quic` | Transport- und Netzwerk-Primitive (`std::net`-Obermenge) |
| `akku::crypto` | `hash`, `aead`, `sign`, `kem`, `kdf`, `rand` | Verhindert Kollision mit `std::hash` + klare Audit-Grenze |
| `akku::time` | `clock`, `datetime`, `duration`, `timezone` | Zusammenhängendes Zeit- und Kalender-Modell |
| `akku::units` | `distance`, `mass`, `time`, `temp`, `energy`, `pressure`, `speed` | Dimensionssystem (`geo` bleibt separat flach!) |
| `akku::math` | `decimal`, `linear`, `special`, `stat` | Numerik-Familie |
| `akku::phy` | `mechanics`, `relativity`, `quantum`, `thermo`, `cosmology` | Physik-Fachrechnungen |
| `akku::actor` | `supervisor`, `mailbox`, `registry` | Erlang-artiges Aktorsystem (`cluster` & `broker` bleiben flach) |
| `akku::gpu` | `compute`, `render` | GPU-Compute & Grafik (inkl. WebGPU auf `wasm32`) |
| `akku::wasm` | `host`, `guest` | **Nur** WASM-Runtime (Host/Plugins/Guest-Helfer) |

---

## Flache Module (Ebene 1)

Alles außerhalb der Familien oben liegt direkt unter `akku::`:

### Netz, Web & Protokolle

| Modul | Inhalt / Regel |
|---|---|
| `akku::http` | `App`/`server`, `client`, `Request`, `Response`, `Status`, `Headers`, `Cookie`, `Path`, `Result`; `http2`/`http3` sind Protokoll-Features innerhalb von `http` |
| `akku::ws` | `server`, `client` (Native + `wasm32`), `Message`, `Frame` |
| `akku::sse` | `server`, `client` (Native + `wasm32`), `Event`, `Stream` |
| `akku::webpush` | WebPush-Client/Server (nicht vages `push`) |
| `akku::url` | URL-/URI-Parsing und Query-Builder |
| `akku::mime` | MIME-Typen und Content-Type-Erkennung |
| `akku::tls` | TLS-Konfiguration, Streams, Zertifikate (`#[doc(alias = "ssl")]`; **kein** eigenes `akku::ssl`-Modul!) |
| `akku::rpc` | `server`, `client` |
| `akku::mail` | SMTP-Client und MIME-Mail-Builder |
| `akku::mqtt` | `client`, `broker` |
| `akku::ssh` | `server`, `client` |

### Markup & Text

| Modul | Inhalt / Regel |
|---|---|
| `akku::html` | Eigenständig getrennt von `akku::http`; `Html`, `escape`, später Parser/Sanitizer |
| `akku::xml` | XML lesen/schreiben |
| `akku::markdown` | CommonMark/GFM → AST/HTML |
| `akku::template` | Sichere Text-/HTML-Templates |
| `akku::regex` | Reguläre Ausdrücke |
| `akku::text` | Unicode, Grapheme, Normalisierung, String-Helfer |
| `akku::diff` | Text-/Struktur-Diff |
| `akku::i18n` | Lokalisierung & Übersetzungen |

### Datenformate, Serialisierung & Codierung

| Modul | Inhalt / Regel |
|---|---|
| `akku::json` | `Value`, `from_str`, `to_string`, typisiertes Mapping |
| `akku::csv` | Streaming Reader/Writer, Header, typisierte Zeilen |
| `akku::toml` | TOML lesen/schreiben |
| `akku::yaml` | YAML lesen/schreiben |
| `akku::ini` | INI-Dateien |
| `akku::dotenv` | `.env`-Dateien |
| `akku::config` | Geschichtete App-Konfiguration (Defaults → Datei → Env) |
| `akku::protobuf` | Protocol Buffers (Runtime + Codegen via `build-dependencies`) |
| `akku::flatbuffers` | Zero-Copy FlatBuffers |
| `akku::msgpack` | MessagePack |
| `akku::cbor` | CBOR |
| `akku::base64` | Base64 & Base64URL |
| `akku::hex` | Hex-Encoding/Decoding |

### Datenbanken, Storage & Daten

| Modul | Inhalt / Regel |
|---|---|
| `akku::sql` | Einheitliche SQL-API, Pool, `migrate`, Treiber (`sql::sqlite`, `sql::postgres` — keine Produktnamen auf Ebene 1!) |
| `akku::kv` | Key-Value-Store-API (embedded + z. B. `kv::redis`) |
| `akku::cache` | In-Memory LRU-/TTL-Cache |
| `akku::dataframe` | Spaltenorientierte Tabellen-Verarbeitung |

### Async, System, CLI & Entwicklung

| Modul | Inhalt / Regel |
|---|---|
| `akku::task` | Async-Tasks, Runtime, Timeout, Cancellation (nicht `async`!) |
| `akku::sync` | Async-/Sync-Channels, Semaphore, Broadcast, Watch (`std::sync`-Obermenge) |
| `akku::schedule` | Intervalle, Cron, Retry/Backoff |
| `akku::cluster` | Knoten-Mitgliedschaft, Gossip, Discovery |
| `akku::broker` | Pub/Sub-Message-Broker |
| `akku::fs` | `std::fs`-Obermenge + `walk`, `glob`, `tempfile`, `watch`, atomisches Schreiben |
| `akku::cli` | Argument-Parser, Subcommands, Auto-Help |
| `akku::term` | ANSI-Farben, Tabellen, Progress-Bars, Prompts |
| `akku::tui` | Terminal-UI-Komponenten |
| `akku::process` | `std::process`-Obermenge + Pipes, Signale |
| `akku::env` | `std::env`-Obermenge + typisierte Vars, OS-Standardpfade |
| `akku::log` | Strukturiertes Logging (Human + JSON) |
| `akku::trace` | Spans, Tracing, Metriken |
| `akku::test` | HTTP-Testclient, Tempdir, Snapshots |
| `akku::bench` | Mikro-Benchmarks |
| `akku::error` | Einheitliches `Error`/`Result` mit Kontext & Error-Chain |
| `akku::bytes` | Byte-Buffer, Endian, Bit-Operationen |
| `akku::id` | UUID (v4/v7), ULID, NanoID |

### Wissenschaft, Medien-Helfer & UI

| Modul | Inhalt / Regel |
|---|---|
| `akku::algo` | Graphen, Pathfinding, Sort-/Search-Algorithmen |
| `akku::geo` | Geokoordinaten, Distanzen, Projektionen (getrennt von `units`) |
| `akku::chem` | Chemie-Domäne (kann bei Wachstum zur Familie werden) |
| `akku::bio` | Biologie-Domäne (kann bei Wachstum zur Familie werden) |
| `akku::econ` | Ökonomie-/Finanz-Domäne (kann bei Wachstum zur Familie werden) |
| `akku::color` | RGB, HSL, OKLCH, Hex, Kontrast |
| `akku::pdf` | PDF erzeugen/lesen |
| `akku::ui` | UI-Komponenten (Native + `wasm32`) |
| `akku::webview` | System-WebView-Anwendungen |

---

## Querschnitts-Konventionen

### 1. `server` und `client` leben *innen*

Protokoll-Module mit Server- und Client-Rolle nutzen immer dieselbe Innenstruktur:

```rust
akku::http::server   // bzw. akku::http::App
akku::http::client   // Native + wasm32 (fetch)

akku::ws::server
akku::ws::client     // Native + wasm32 (WebSocket)

akku::sse::server
akku::sse::client    // Native + wasm32 (EventSource)
```

Gemeinsame Typen (`Request`, `Response`, `Message`, `Event`, `Status`) liegen
direkt am Protokoll-Modul (`akku::http::Request`, `akku::ws::Message`).

### 2. `wasm32`-Target vs. `akku::wasm`

- **Kein `forwasm::`-Modul.** Client- und reine Daten-Module unterstützen
  `wasm32` transparent hinter derselben API via `#[cfg(target_arch = "wasm32")]`.
- **`akku::wasm`** ist ausschließlich die WASM-Runtime (`wasm::host`, `wasm::guest`).

### 3. `std`-Namens-Obermengen

Module, die denselben Namen tragen wie ein `std`-Modul (`fs`, `net`, `sync`,
`time`, `env`, `process`), sind als **Obermenge** gedacht, sodass `use akku::fs`
ergonomisch neben oder statt `std::fs` genutzt werden kann.

### 4. Keine Produktnamen auf Ebene 1

Anbieter- und Produktnamen leben unter ihrer fachlichen Domäne:
- `akku::kv::redis` (nicht `akku::redis`)
- `akku::sql::postgres`, `akku::sql::sqlite` (nicht `akku::postgres`)

### 5. Reservierte generische Typnamen

Diese Typnamen tauchen niemals direkt unter `akku::` auf, sondern immer in
ihrem Modul:

```text
Server, Client, Request, Response, Error, Result, Value, Map, Set,
List, Node, Element, Document, Route, Handler, Context, Config
```

### 6. Makros

Makros folgen derselben Modulregel:

```rust
#[akku::http::get("/users/{id}")]
#[akku::http::post("/users")]
#[akku::http::main]
```

Nicht `#[akku::get]` oder `#[akku::main]`. `akku-drive` bleibt ein internes
Implementierungsdetail; Nutzer importieren nur `akku`.

### 7. Prelude

Ein optionales `akku::prelude` darf Bequemlichkeit bieten, übernimmt aber keine
Namenshoheit. Der vollständige Modulpfad bleibt immer die kanonische Quelle.
