# Post

A Gmail-inspired, keyboard-first terminal email client built in Rust with
Ratatui, backed by the [Himalaya CLI](https://pimalaya.org) for all mail
protocols, accounts, and credentials.

**Status: Phase 0 (backend discovery) complete.** Implementation proceeds
phase by phase per `POST_IMPLEMENTATION_PLAN.md`.

## Requirements

- macOS (required) / Linux (supported); Windows is out of scope
- Stable Rust (2024 edition)
- `himalaya` CLI v2.x installed and configured (`himalaya configure`)

## Development

```sh
cargo build
cargo run --bin probe -- <config.toml>   # Phase 0 backend probe harness
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

## Repository layout

- `docs/adr/` — architecture decision records
- `fixtures/himalaya/` — sanitized probe fixtures, schemas, seed/sink helpers
- `src/bin/probe.rs` — subprocess probe (argv-only, stdin, cancellation)
- `POST_IMPLEMENTATION_PLAN.md` — the product/engineering specification

## License

TBD
