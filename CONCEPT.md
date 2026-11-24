# Akku — Konzeptpapier

Stand: 2026-11-21  
Repo/Workspace: `akku`  
Produkt/Crate: `akku`

## 1. Vision

Akku ist eine **expansive Standardbibliothek neben `std`** für Rust.

Zielgefühl:

> Ich starte ein reales Rust-Projekt, meine Abhängigkeiten sind `akku` und `std` — und alles, was ich für Web, CLI, Datenformate, System, Medien oder Nebenläufigkeit brauche, ist da, konsistent und angenehm.

```toml
[dependencies]
akku = "0.2"
```

### Langfristiges Ziel: 0 externe Dependencies

Das ultimative Ziel von Akku ist **Zero External Dependencies**:

- Externe Crates (`adapter`) sind ausdrücklich nur ein **temporärer Speedup**, um früh nutzbar zu sein (wie aktuell `axum`/`tokio` in `akku::http`).
- Hinter derselben stabilen Akku-API werden Adapter schrittweise durch **Eigenbau (`native`)** oder **lizenzsaubere, verschlankte Code-Forks (`fork`)** abgelöst — ohne Breaking Change für Nutzer.
- Bei sicherheitskritischen Kernen (`crypto`, `tls`) gilt besondere Sorgfalt: kein blinder Schnell-Eigenbau, sondern geprüfte, auditierte Forks (MIT/Apache-2.0/BSD/ISC/MPL-2.0 mit sauberer Attribution) oder gepinnte Adapter, bis ein voll auditierter Stand existiert.

## 2. Nicht-Ziele

- Keine Ersetzung von Rusts `core`/`alloc`/`std` — Akku baut auf `std` auf und erweitert es.
- Keine 20+ internen oder 300 externen Crates: **Es gibt nur eine Library-Crate (`akku`).**
- Keine reine 1:1-Weiterleitung fremder APIs (wie `axum`/`serde`/`tokio`) als öffentlicher Vertrag.
- Keine Platzhalter-Crates auf crates.io.
- Kein GPL-/AGPL-Code in Forks (Akku steht unter MPL-2.0).

## 3. Produktform: Eine Library-Crate

```text
akku/
  crates/
    akku/        # die eine Library-Crate (alle Module leben unter src/)
    akku-drive/  # proc-macro = true (von Rust erzwungen; von akku re-exportiert)
    akku-cli/    # optionales Binary: `akku new <name>`
```

- Nutzer binden immer nur `akku` ein (auch für Codegen in `[build-dependencies]`, z. B. bei `protobuf`/`flatbuffers`, statt neuer Crates).
- **Ownership ohne Extra-Crates:** Jedes Modul liegt in einem eigenen Ordner unter `crates/akku/src/<modul>/` mit eigener Metadatei (`meta.toml`) und ordnerbasierten `CODEOWNERS`.
- **Merge-Konflikte vermeiden:** Ein Generator-Script (in CI geprüft) erzeugt aus den Modul-Metadaten die `src/modules.rs`-Registrierung, `CODEOWNERS` und den Tag-Katalog (z. B. GitHub Pages).

## 4. Namespace-Prinzip: Flach als Default + dynamische Familien

Die vollständigen Regeln stehen in [`Namespace.md`](Namespace.md). Kurzfassung:

1. **Alles flach als Default** (`akku::http`, `akku::ws`, `akku::json`, `akku::csv`, `akku::tls`, `akku::sql`, `akku::fs`, `akku::cli`, `akku::base64`, `akku::protobuf`, `akku::flatbuffers`, …).
2. **Dynamische Familien:** Wenn zusammengehörige Module organisch wachsen, werden sie als Familie gruppiert (vor `1.0` direkt; ab `1.0` über `#[deprecated] pub use` Übergangs-Aliase).
3. **Kurze, prägnante Namen** für Familien und Module:
   - `archive/`, `auth/`, `audio/`, `vid/`, `pic/` (Medientypen strikt getrennt: eine DAW braucht kein `pic`/`vid`, ein Bildeditor kein `audio`)
   - `net/`, `crypto/`, `time/`, `units/`, `math/`, `phy/`, `actor/`, `gpu/`, `wasm/`
4. **Server/Client immer innen** (`akku::http::{server, client}`, `akku::ws::{server, client}`, `akku::sse::{server, client}`).
5. **Discovery über Tags statt tiefer Taxonomie:** Querschnitts-Sichten (`#web`, `#cli`, `#wasm32`, `#native-0dep`, `#adapter`, `#no_std`, `#maintainer:...`) liefert ein Katalog (GitHub Pages), nicht ein 4-stufiger Modulbaum.

## 5. Minimal-Features & Compile-Zeiten

Der Rust-Linker + LTO entfernt ungenutzten Code aus dem fertigen Binary. Features werden daher **minimal** eingesetzt — nicht pro Kleinstmodul, sondern als Schutzplanke für Compile-Zeit, Plattform-Builds (`wasm32`) und Übergangs-Adapter:

- **Zero-Dep-Alltagsmodule** (`error`, `bytes`, `id`, `base64`, `hex`, `url`, `html`, `json`, `csv`, `toml`, `units`, `math`-Core, …) sind ohne Feature-Gefummel direkt nutzbar.
- **Schwere, OS-abhängige oder noch über `adapter` laufende Domänen** (`http`, `ws`, `sql`, `tls`, `crypto`, `auth`, `archive`, `audio`, `vid`, `pic`, `gpu`, `wasm`, …) hängen hinter groben Domänen-Features (gesamt ~30–60 Features, weit unter dem crates.io-Limit von 300).
- **Komfort-Bundles** (`web`, `cli`, `data`, `sci`, `full`) bündeln typische Projekt-Setups.
- `cargo tree -e features -p akku` bleibt die Kontrolle für den tatsächlichen Dependency-Graph.

## 6. WASM-Modell (strikt zweigeteilt)

1. **Code, der *in* WASM läuft (`wasm32`-Target):**
   - **Kein** eigenes `forwasm::`- oder `async::wasm`-Modul.
   - Client- und Compute-Module (`http::client`, `ws::client`, `sse::client`, `gpu`, `json`, `url`, `time`, `units`, `math`, `pic`, …) unterstützen `wasm32` transparent hinter derselben API via `#[cfg(target_arch = "wasm32")]`.
   - Native-only Server-/OS-Teile (`http::server`, `ws::server`, `net::tcp`, `process`, `fs::watch`) werden auf `wasm32` automatisch ausgeblendet.
2. **Code, der *selbst* WASM-Module ausführt (Host/Guest-Runtime):**
   - Lebt sauber im eigenen Modul `akku::wasm` (`wasm::host`, `wasm::guest`).

## 7. Backend-Lifecycle & Lizenzpolitik

Jedes Modul trägt explizit einen Backend-Status:

| Status | Bedeutung | Beispiel |
|---|---|---|
| `native` | 0 externe Crates, reiner Akku-Code auf `core`/`alloc`/`std` | `html`, `bytes`, `error`, `base64`, `csv`, `json`, `toml`, `units`, `math` |
| `fork` | 0 externe Crates, gezielt übernommener & gepflegter Code im Repo | `crypto`, `tls`, `regex`, `archive`, `pic` |
| `adapter` | Temporäre externe Crate (exakt gepinnt) als Speedup bis Ablösung | `http` (v0 auf `axum`/`tokio`), frühe DB-/Audio-/GPU-Anbindung |

Regeln:

- Temporäre Adapter-Dependencies werden **exakt gepinnt** (`axum = "=0.8.9"`), über `cargo deny`/`cargo audit` geprüft und hinter Akkus eigener API versteckt.
- Code-Forks stammen ausschließlich aus MPL-2.0-kompatiblen Lizenzen (MIT, Apache-2.0, BSD, ISC, MPL-2.0) mit dokumentierter Herkunft und Lizenztext in `THIRD_PARTY_LICENSES`.
- Sicherheitskritische Kerne (`crypto`, `tls`) werden nicht ungetestet aus Prestige neu geschrieben; hier haben auditierte Forks/Adapter Vorrang.

## 8. API-Prinzipien

1. **User-first.** Erst den Nutzercode entwerfen, dann die Implementierung.
2. **Ein offensichtlicher Weg.** Kein Trait-Labyrinth für den Normalfall.
3. **Austauschbare Backends.** Die öffentliche Akku-API bleibt stabil, wenn ein Modul von `adapter` auf `native` oder `fork` wechselt.
4. **Escape-Hatch wo nötig.** Solange ein Adapter darunter liegt (z. B. `App::into_axum`), bleibt der Ausstieg explizit dokumentiert.
5. **Makros sind Zucker, keine Voraussetzung.** Die manuelle API muss ohne Makros angenehm sein.
6. **Defaults sicher und performant.** Kein unsicheres Legacy-Modul auf Top-Level (z. B. kein `akku::ssl`).
7. **Vermeidung von Rust-Keywords.** Keine reservierten Schlüsselwörter als Modulnamen (`task` statt `async`, kein `gen`, `try`, `box`).

## 9. Erfolgskriterien

Akku ist erfolgreich, wenn:

- ein reales Projekt nur `akku` + `std` in der `Cargo.toml` braucht,
- Welle-1-Alltagsmodule (`html`, `json`, `csv`, `toml`, `url`, `base64`, `hex`, `cli`, `log`, `time`, `units`, `math`, …) mit **0 externen Dependencies** laufen,
- die öffentliche API beim Wechsel von `adapter` → `native`/`fork` stabil bleibt,
- Client-Code (`http::client`, `ws::client`, `json`, `units`, …) ohne Sonder-Imports auch auf `wasm32` kompiliert,
- `cargo test`, `cargo clippy`, `cargo doc` und `cargo deny`/`audit` grün sind,
- die gesamte Bibliothek über eine klare Roadmap ([`docs/ROADMAP.md`](docs/ROADMAP.md)) von ~58 Kernmodulen sauber auf 300+ wachsen kann.
