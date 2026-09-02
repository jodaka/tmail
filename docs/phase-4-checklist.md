# Phase 4 — Single-message reader and core actions: checklist

Status: COMPLETE (2026-09-02)

## Scope delivered (plan §19 Phase 4, kata issue qa3c)

- `src/domain/message.rs` — `MessageLocator { mailbox, id, message_id }`
  (serde, for typed retry intents and stale-result rejection), 
  `MessageHeaders` (subject/from/to/cc/date/message-id), `Attachment`
  (name/mime/size/part-id metadata only — saving is Phase 8), `Message`
  with `plain_body`/`html_body` and a `snippet()` helper; `MessageSummary`
  gained the plan §7 `to` field (envelope rows now map recipients).
- `src/backend/traits.rs` — five new `MailBackend` operations:
  `get_message`, `set_read`, `set_starred`, `archive`, `trash` (plan §8).
- `src/backend/himalaya/` — argv builders `message read -m <m> <id>
  --json`, `flag {add,remove} -m <m> --flag <f> <id> --json`,
  `message move --from <s> --to <t> <id> --json`, `message delete -m <m>
  <id> --json`; mail_parser serde-dump DTOs (`parts`/`text_body`/
  `html_body`/`attachments`, externally tagged header values and bodies
  behind untagged leniency wrappers, mail_parser's `address` field spelling
  and `message_id` underscore header handled); mapper `map::message` with
  offset-sign date conversion and attachment metadata extraction.
  `HimalayaCliBackend` now caches the last `mailbox list` result
  (`Arc<RwLock>`) so `archive` resolves its target *inside the adapter*
  from the cached listing's Archive role, then the `archive` alias —
  never in the UI (ADR 0001); trash delegates to trash-first
  `message delete`.
- `src/app/operation.rs` — `OperationKind::{LoadMessage, SetRead,
  SetStarred, Archive, Trash}` with status-bar summaries and supersede
  rules (same-mailbox message loads, same-message flag toggles; move
  operations never supersede — a lost archive is unrecoverable);
  `OperationOutcome::{Message(Box<Message>), Done}`.
- `src/app/route.rs` / `state.rs` — `Route::Message(MessageRoute)`
  carrying the summary snapshot; `AppState.open_message: Loadable<Message>`
  (new `Idle` variant) and `reader_scroll`; `action_target()` resolves the
  reader/list target for shortcuts.
- `src/app/reducer.rs` — Enter opens the reader (route push + `LoadMessage`
  + untouched list underneath); reader Up/Down scroll the document,
  Left/Right page it (scroll clamp shared with the renderer's content
  function, Phase 3 modal pattern); successful load fills missing list
  snippets and starts a separate retryable `SetRead(true)` for unread mail;
  flag/move results apply only on confirmation (list row *and* reader
  snapshot update together); confirmed archive/trash removes the row, keeps
  the selection index, closes the reader when it showed the moved message,
  and re-syncs the page (maildir ids change on move, ADR 0001 finding 4);
  failures open the Retry/Dismiss modal with nothing mutated; reader retry
  resets the loading placeholder; `Esc` order unchanged (cancel → overlay →
  back) and popping the reader restores page/selection/focus/scroll by
  construction.
- `src/ui/screens/reader.rs` — one-message reader from `viewer.html` (plan
  §4 overrides: no thread count, no collapsed messages, no thread
  navigation, no tags): subject, From/To/Cc/Date meta (loaded-message
  headers win over the summary snapshot), action row, hairline,
  width-wrapped body, attachment chips; placeholders for missing
  subject/sender/recipients/date/body; HTML-only mail degrades to a clear
  note (rich rendering is Phase 5); loading/failed/idle states render.
- `src/ui/mod.rs`, `statusbar.rs` — route dispatch renders the reader in
  the list area with sidebar/topbar chrome kept; ` READER ` mode badge and
  reader key hints.
- `src/ui/layout.rs` — `reader_rows_visible`/`reader_width` (shared
  reducer/renderer viewport math); `src/ui/dates.rs` — `format_absolute`
  (`Mon, Sep 1 · 18:32`).
- `tests/fake_himalaya.rs` — fake now serves `message read/move/delete`
  and `flag add/remove` with per-mode behavior (ok/error-json/slow),
  recording exact argv.
- `tests/backend_contract.rs` — 7 new tests: exact argv for read /
  flag add+remove / move (target resolved from the cached listing) /
  delete; `get_message` fixture mapping; typed failures; archive without a
  known target rejected before spawning.
- `src/app/reducer_tests.rs` — 15 new tests; `tests/terminal_snapshots.rs`
  — 5 new render tests.
- `target/probe/smoke_phase4.py` — live pty smoke (scratch, uncommitted).

## Acceptance criteria (plan §19 Phase 4)

- [x] Reader shows exactly one message — reducer test
      `activate_on_message_list_opens_the_reader`; snapshot
      `reader_renders_exactly_one_message_document` (asserts no `3 of 3`,
      no thread navigation; READER badge; body present)
- [x] Reader handles missing subject/from/body — reducer/snapshot
      `reader_handles_missing_fields` (`(no subject)`, `(unknown sender)`,
      `(no recipients)`, `unknown date`, `(no content)`),
      `html_only_mail_degrades_to_a_note`, loading/failed/idle tests
- [x] Back restores page, selection, focus, and scroll — reducer test
      `esc_from_reader_restores_exact_list_state` (list page, selection,
      `list_scroll`, focus asserted against a pre-open clone; restoration
      is by construction — the list is never mutated while the reader is
      open); `esc_cancels_message_load_then_second_esc_returns` covers the
      §10 Esc order mid-load
- [x] Actions update UI only after confirmation initially — flag results
      apply only on `Ok(Done)` (`reader_result_applies_and_marks_unread_
      read`, `toggle_star_from_list_requests_inverse_and_applies_on_
      confirmation`, `mark_unread_updates_list_and_route_after_
      confirmation`); archive/trash remove rows only on `Ok(Done)`
- [x] Failure leaves a coherent state and opens Retry/Dismiss —
      `message_load_failure_opens_modal_and_retry_replays` (reader stays,
      failed placeholder, retry replays the typed `LoadMessage` intent
      under a new id), `archive_failure_keeps_row_and_opens_modal` (row
      kept, no reload)

## Commands run / results

- `cargo fmt --all` — clean
- `cargo check --all-targets --all-features` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-features` — 201 passed, 0 failed
  (156 unit incl. 15 reducer + reader/DTO/map tests + 27 contract +
  2 fake-himalaya + 16 snapshots)
- Real-CLI pty smoke (`himalaya` 2.1.0, probe maildir, Python pty driver
  at 152×40, `target/probe/smoke_phase4.py`; maildir wiped before seeding —
  the seed script appends): startup rows → Enter opens the reader with
  subject/meta/body/actions/READER badge → Esc restores the list → `s`
  star toggle confirmed → `e` archive removes the row and re-syncs →
  clean quit with a single `?1049h`/`?1049l` pair. Post-run maildir
  proves the flags: opened message `seen`, unstarred; archived message
  gone from INBOX into the Archive folder.

## Corrections discovered by the live smoke

- ADR 0001 finding 6 corrected: without `--json`, `flag {add,remove}` and
  `message {move,delete}` print **human text** (`Successfully added flags:
  flagged`) with exit 0; the Phase 0 schema shape (`{"flags":[…]}`) only
  appears with `--json`. All mutation argv builders now pass `--json`
  (the smoke caught the first run misclassifying a *successful* flag
  change as a failure — the Retry/Dismiss modal worked exactly as
  designed).

## Notes for Phase 5+

- Sidebar unread counts are not recomputed after flag changes (rows are);
  they refresh on the next mailbox listing (manual refresh / Phase 9).
- `message read` supports `--seen`; Post deliberately marks read as a
  separate retryable operation instead (one typed intent per operation).
- The reader renders plain text only; HTML-to-RichText is Phase 5
  (`html_body` is already carried in `Message`).
- Attachment chips are metadata-only; bytes flow in Phase 8 via
  `attachment download <part-id>` (part ids already captured).
- `Loadable::Idle` is available for the composer phases' non-open slots.
