# Contributing

## Setup

```sh
cargo test --workspace --all-features
cargo test -p akku --no-default-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

`rustfmt` and `clippy` are required for a pull request; CI runs both.

## Changes

- One topic per pull request.
- Every user-visible change gets a [changie](https://changie.dev/) fragment:
  `changie new` or a YAML file in `.changes/unreleased/`.
- New public names must follow [`Namespace.md`](Namespace.md).
- New runtime behaviour needs a test.
- New API needs a user-first example in `examples/`.
- Do not add a global `akku::get` or `akku::Value`.

## Security

- Do not add long-lived crates.io tokens to the repository or to GitHub
  secrets. Releases use Trusted Publishing (OIDC).
- Do not add `pull_request_target` workflows that expose secrets.
- Pin third-party actions to a commit SHA when touching the workflows.

## License

By contributing you agree that your contribution is licensed under
MPL-2.0.
