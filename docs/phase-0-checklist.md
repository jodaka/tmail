# Phase 0 — Repository and backend discovery: checklist

Status: COMPLETE (2026-09-02)

## Tasks

- [x] Inspect existing repository; initialize one binary crate (was empty;
      `cargo init`, edition 2024, Rust 1.95.0)
- [x] Record supported Rust and Himalaya versions
      (Rust 1.95.0; himalaya 2.1.0 +smtp +imap +jmap +gmail +msgraph +maildir)
- [x] Minimal CLI probe / integration harness (`src/bin/probe.rs`)
- [x] Discover structured commands/JSON for mailboxes, envelopes, message
      retrieval, flags, search, attachments, draft storage, send, reply,
      forward (ADR 0001 table)
- [x] Generate JSON Schemas (`himalaya json-schema` → 70 schemas,
      `fixtures/himalaya/schemas/`)
- [x] Verify `[post]` keys are tolerated in the same TOML file (yes, no
      warnings; ADR 0001 finding 13)
- [x] Prove child-process cancellation (~150 ms SIGKILL latency, argv-only,
      no shell; `src/bin/probe.rs` step 3)
- [x] Prove draft create/update/replace/delete behavior → no in-place update
      exists; trash-first delete confirmed; add-then-delete strategy chosen
      (ADR 0002)
- [x] Characterize send success vs Sent-copy failure vs ambiguous outcomes
      (`fixtures/himalaya/send-outcomes.md`; SentButCopyFailed is
      pre-validated away by himalaya, post-DATA EOF reproduced as Unknown)
- [x] Write ADRs: backend boundary (0001), draft strategy (0002)

## Acceptance criteria (plan §19 Phase 0)

- [x] A test/probe invokes Himalaya without shell interpolation
      (`src/bin/probe.rs`: argv arrays only, run verified 2026-09-02)
- [x] Representative JSON is captured and secrets are removed
      (`fixtures/himalaya/*.json`; probe account uses `probe@post.local`,
      no credentials anywhere)
- [x] Draft replacement has a documented safe algorithm (ADR 0002)
- [x] Config coexistence has a documented solution (ADR 0001 finding 13;
      `config.example.toml`)
- [x] Ambiguous send outcomes can be identified or conservatively classified
      as unknown (`fixtures/himalaya/send-outcomes.md` classification table)

## Commands run / results

- `cargo fmt --all` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-targets --all-features` — 0 tests yet, harness green
- `cargo run --bin probe -- target/probe/config.toml` — all 3 steps pass
- `python3 fixtures/himalaya/seed_maildir.py` — reproducible Maildir env
- `python3 fixtures/himalaya/smtp_sink.py 3425 ok|silent-drop` — send probe

## Notes for Phase 1+

- `envelope list` gives no snippet and no totals on maildir; UI must degrade.
- Message ids change on move (maildir); selection stability keys on
  `Message-ID`.
- Shared commands default to the inbox alias mailbox; always pass `-m`.
- `--json` output has no trailing newline; errors are JSON on stdout.
