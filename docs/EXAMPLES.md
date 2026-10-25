# Examples

Every shipped feature has a runnable example. Examples are `publish = false`
and part of the workspace.

| Feature | Example | Run | Status |
|---|---|---|---|
| HTML | `examples/hello` | `cargo run -p hello` | existing |
| Attribute routes | `examples/hello` | `cargo run -p hello` | existing |
| `akku::http::main` | `examples/hello` | `cargo run -p hello` | existing |
| CLI scaffolder | `akku-cli` | `akku new hello` | existing |
| JSON | `examples/json` | `cargo run -p json-example` | planned |
| Units | `examples/units` | `cargo run -p units-example` | planned |
| Math | `examples/math` | `cargo run -p math-example` | planned |
| Streaming/SSE | `examples/sse` | `cargo run -p sse-example` | planned |
| dree adapter | `examples/dree` | `cargo run -p dree-example` | planned |

Rules:

- A feature is only done when its example compiles and runs.
- Examples use only `akku` plus `std` unless the example is explicitly about an
  integration.
- Each example has a short README with the command and expected URL/output.
