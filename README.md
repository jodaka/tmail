# Post

A Gmail-inspired, keyboard-first terminal email client built in Rust with
Ratatui, backed by the [Himalaya CLI](https://pimalaya.org) for all mail
protocols, accounts, and credentials.

**Status: Phase 2 (Himalaya adapter + Inbox vertical slice) complete.** The
app browses real mailboxes and pages real messages through the Himalaya
CLI. Implementation proceeds phase by phase per
`POST_IMPLEMENTATION_PLAN.md`; see `docs/phase-2-checklist.md`.

## Running

```sh
cargo run --release                     # himalaya's default config
cargo run --release -- path/to/config.toml
POST_CONFIG=path/to/config.toml cargo run --release
```

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
- `docs/phase-*.md` — per-phase delivery checklists
- `fixtures/himalaya/` — sanitized probe fixtures, schemas, seed/sink helpers
- `src/backend/` — `MailBackend` trait + Himalaya CLI adapter (DTOs private)
- `src/config/` — shared one-file configuration (`[post]` + aliases)
- `src/bin/probe.rs` — subprocess probe (argv-only, stdin, cancellation)
- `tests/fake_himalaya.rs` — fake `himalaya` executable for contract tests
- `tests/backend_contract.rs` — backend contract test suite
- `POST_IMPLEMENTATION_PLAN.md` — the product/engineering specification

## License

TBD
