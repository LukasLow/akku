# Security

## Supported versions

Akku is pre-1.0. Security fixes target the latest `0.0.x` release.

## Reporting

Do not open a public issue for a vulnerability. Report it privately to the
repository owner with:

- affected crate and version,
- a minimal reproduction,
- impact and any known mitigations.

## Security posture

- `akku` forbids unsafe code (`#![forbid(unsafe_code)]`).
- The v0.0.1 HTTP backend is axum 0.8/tokio; TLS and crypto are not implemented
  by Akku itself.
- Dependencies are pinned in the workspace and reviewed before release.
- `akku::html::Html` is explicitly trusted markup. User text must go through
  `akku::html::escape` or be interpolated as escaped text by the caller.
- No authentication, session or TLS API is part of v0.0.1.

## Dependency policy

The goal is a coherent API, not dependency purity. Security-critical primitives
remain in established, audited crates. New direct dependencies need a reason,
an exact version and a review of their feature impact.
