# Development

## Module map

The module map — one page on the layers and data flow (reducer loop,
backend trait, UI/input/config slices) — is [architecture.md](architecture.md).

## Building and testing

```sh
cargo build
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
python3 fixtures/smoke/ci_smoke.py --bin target/debug/tmail   # pty smoke (CI runs it too)
```

## Repository layout

- `docs/adr/` — architecture decision records
- `docs/phase-*.md` — per-phase delivery checklists
- `.github/workflows/ci.yml` — macOS + Linux CI (fmt, clippy, tests, pty
  smoke) plus a Windows job (fmt, clippy, tests)
- `fixtures/himalaya/` — sanitized probe fixtures, schemas, seed/sink helpers
- `fixtures/smoke/` — committed pty smoke: fake himalaya + CI driver
- `src/backend/` — `MailBackend` trait + Himalaya CLI adapter (DTOs private)
- `src/config/` — shared one-file configuration (`[tmail]` + aliases)
- `src/input/` — keyboard and mouse → action translation
- `tests/fake_himalaya.rs` — fake `himalaya` executable for contract tests
- `tests/backend_contract.rs` — backend contract test suite
- `TMAIL_IMPLEMENTATION_PLAN.md` — the product/engineering specification
