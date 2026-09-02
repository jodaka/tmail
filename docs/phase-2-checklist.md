# Phase 2 — Himalaya adapter and first Inbox vertical slice: checklist

Status: COMPLETE (2026-09-02)

## Scope delivered (plan §19 Phase 2, kata issue zbv0)

- `src/backend/traits.rs` — `MailBackend` trait (`async_trait`, dyn-compatible
  for the Phase 3 operation manager) + typed `BackendError`
  (`Command`/`InvalidOutput`/`InvalidRequest`/`Io`). Request context,
  cancellation, and operation ids arrive with Phase 3.
- `src/backend/himalaya/` — `HimalayaCliBackend` on the installed CLI:
  - `process.rs` — argv-only `tokio::process::Command` (no shell), piped
    stdout/stderr drained by dedicated tasks, `kill_on_drop(true)`, stdin
    null for read-only commands; exit-status-authoritative decoding per
    ADR 0001 finding 1; malformed/non-UTF-8 output → typed
    `InvalidOutput`, never a panic
  - `dto.rs` — Himalaya-private DTOs mirroring the Phase 0 schemas
    (lenient `#[serde(default)]` for schema-optional fields; IANA flags
    read as plain strings so future flags cannot break parsing)
  - `map.rs` — pure DTO→domain mappers; mailbox roles resolved inside the
    adapter from `mailbox.alias` first, then well-known names (ADR 0001);
    missing `Date` → Unix-epoch fallback; snippet stays `None` (finding 2)
  - `command.rs` — argv builders; 0-based `PageRequest{offset,limit}` maps
    to 1-based `-p = offset/limit + 1`, `-s = limit` (finding 8)
- `src/config/` — one shared TOML file (ADR 0001 finding 13): `[post]`
  account/page_size plus the selected account's `mailbox.alias` table;
  resolution order CLI arg → `POST_CONFIG` → `~/.config/himalaya` →
  `~/Library/Application Support/himalaya`; unreadable/malformed files
  fall back to defaults while still forwarding `-c` to himalaya
- `src/app/effect.rs` — typed effects; `reduce` now returns `Vec<Effect>`
  (plan §5: effects launch typed backend requests)
- `src/app/action.rs` — `MailboxesLoaded` / `PageLoaded` result actions
  (stand-ins for Phase 3's `BackendCompleted(OperationResult)`)
- `src/app/state.rs` — startup state is empty (`initial(page_size)`):
  mailboxes `Loading`, no route; `pending_page` scalar guard so stale
  results never apply; `list_scroll` render anchor
- `src/app/reducer.rs` — mailboxes loaded → select Inbox role, else first
  mailbox; page requests/results; selection re-resolved by `Message-ID`
  then backend id (ADR 0001 finding 4); failures keep the last coherent
  page and report via the status line (modal arrives in Phase 3)
- `src/ui/layout.rs` — `message_rows_visible(size)` shared by renderer and
  reducer; `screens/mailbox.rs` renders from the scroll anchor so the
  selection is always on screen; `sidebar.rs` renders Loading/Failed/empty
- `src/main.rs` — real backend wiring: config → `Arc<dyn MailBackend>`,
  result channel, effect dispatch inside the `tokio::select!` loop
- `src/domain/` — `PageRequest` added; `MessageSummary.message_id`;
  `Page::has_next` degrades to "full page ⇒ next may exist" when the
  total is unknown (maildir), plan §16
- `tests/fake_himalaya.rs` — fake `himalaya` executable (bash script in a
  temp dir, behavior baked in, records exact argv/stdin, no env mutation)
- `tests/backend_contract.rs` — 18 contract tests (see acceptance below)
- `mock.rs` stays as the deterministic fixture for reducer tests and UI
  snapshots; the live app no longer consumes mock data

## Acceptance criteria (plan §19 Phase 2)

- [x] Himalaya DTOs are not referenced by app/UI modules — enforced by
      `app_and_ui_modules_never_reference_backend_or_dtos`, which scans
      every `src/app/**` and `src/ui/**` source line (prose excluded) for
      backend/dto references
- [x] Empty, partial, malformed, and non-UTF-8-ish output paths fail
      safely — contract tests: empty page, partial envelope (`{"id":…}`
      only), malformed text, non-UTF-8 bytes, JSON-error-on-stdout with
      exit 1, stderr-only errors, zero page limit rejected without
      spawning a child; mapper unit tests cover missing optional fields
      (subject/addresses/date/message-id) with safe defaults
- [x] Selection remains visible and valid across movement and resize —
      reducer tests: window follows selection at 20-row terminals, moving
      up does not shift the window until the top edge, resize shrinks the
      window, scroll clamps when pages shrink, out-of-range selection
      normalizes; `message_rows_visible` shares the renderer's exact
      layout math; verified live in a 40-row pty
- [x] First/last page boundaries do not issue invalid requests — reducer
      tests: no request before page 1, none past a known total, none when
      the current page is short with an unknown total; out-of-range pages
      that do get issued return `{"envelopes":[]}` and apply as a valid
      empty page (verified against the real CLI: `-p 99` exits 0)
- [x] Fake backend tests assert exact argv/stdin handling —
      `mailbox_list_argv_is_exact`, `envelope_list_argv_is_exact…`,
      `argv_without_config_or_account_is_minimal`,
      `mailbox_ids_with_spaces_stay_single_argv_entries` (no shell),
      `read_only_commands_receive_no_stdin`

## Commands run / results

- `cargo fmt --all` — clean
- `cargo check --all-targets --all-features` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-features` — 105 passed, 0 failed
  (79 unit + 18 contract + 2 fake-himalaya + 6 render snapshots)
- Real-CLI pty smoke (himalaya 2.1.0, probe maildir, `page_size = 2`,
  Python pty driver at 152×40):
  - startup renders mailboxes (alias-resolved roles) and page 1 of INBOX
    with correct senders/subjects/dates/flags; `-p 1 -s 2` on the wire
  - Right/Right/Right/Left issued exactly `-p 2/3/4/3` with the pending
    base advancing on rapid keys; range labels followed (`1–2`, `3–4`,
    `5–6`, `7–8`); an out-of-range page applied as a valid empty page
  - Tab Tab ↓ ↓ Enter switched to Sent (empty mailbox handled safely)
  - live resize to 100×30 re-rendered compact without panic
  - Ctrl+C exited 0 with a single `?1049h`/`?1049l` pair; a 0×0 pty
    degrades to the too-small message

## Notes for Phase 3+

- The result channel + effect dispatch in `main.rs` is the minimal seed of
  the operation manager: `pending_page` (scalar stale guard), no
  cancellation, string errors. Phase 3 replaces them with the registry,
  `OperationId`, kill-on-cancel, and the Retry/Dismiss modal.
- `Refresh` during an in-flight page load refreshes the displayed offset;
  the result may then be dropped as stale. Acceptable until the registry.
- Sidebar `Loadable::Failed` shows a dim note; the error modal supersedes.
- Maildir message counts grew across seed runs (ties unordered per ADR
  0001 finding 3) — re-seed fresh before fixture-sensitive checks.
