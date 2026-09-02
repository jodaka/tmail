# Phase 3 — Operation manager, cancellation, and error modal: checklist

Status: COMPLETE (2026-09-02)

## Scope delivered (plan §19 Phase 3, kata issues 4sny / 8x71 / j0ne / 70gq / adkg / 57kq / yn94)

- `src/app/operation.rs` — `OperationId` (opaque, registry-allocated),
  `OperationKind` (typed intent), serializable `RetrySpec` (plan §12: no
  closures), `Operation` (id/kind/started_at/retry/cancellation token), and
  `OperationRegistry`: fresh token per operation, same-kind supersede
  (mailbox loads globally, page loads per mailbox) which cancels and removes
  the older request so its late result can never apply, `cancel_foreground`
  for `Esc`, `page_in_flight`/`is_loading_mailboxes` queries. Result types
  `OperationOutcome`/`OperationFailure`/`OperationResult` form the task
  result channel payload (app-owned; no backend types leak in).
- `src/app/effect.rs` — `Effect { id, kind }`: every request carries the
  `OperationId` allocated by the registry (plan §5/§11).
- `src/app/action.rs` — `BackendCompleted(OperationResult)` replaces the
  Phase 2 stand-in result actions, matching plan §9.
- `src/app/state.rs` — `operations: OperationRegistry` +
  `overlay: Option<Overlay>`; the scalar `pending_page` guard is gone (the
  registry supersedes it).
- `src/app/overlay.rs` — `Overlay::Error(ErrorDialog)`: exit code, sanitized
  detail, typed retry, ambiguity flag, scroll, focused button, previous
  focus.
- `src/app/sanitize.rs` — dependency-free redaction of labeled secrets
  (`password=`, `"token": …`), `Authorization`/`Bearer` credentials, and
  URL userinfo passwords, before anything reaches logs or the UI (plan §12;
  security checklist item 2).
- `src/app/reducer.rs` — modal interception of all input while open
  (Up/Down scroll, Left/Right page-scroll, Tab toggles buttons, Enter
  activates the focused button, `Esc`/Dismiss closes and restores focus,
  Retry replays the typed intent under a *new* `OperationId`); results apply
  only while the operation is still registered and targets the active
  mailbox (unknown/cancelled/superseded results are dropped, plan §11);
  `Esc` order per plan §10: cancel foreground work → close overlay → back;
  `Refresh` starts (or skips if already running) the mailbox listing before
  any route exists, so startup flows through the same reducer path.
- `src/backend/traits.rs` — `RequestContext { operation, cancellation }` on
  every backend call (plan §8) + typed `BackendError::Cancelled`.
- `src/backend/himalaya/process.rs` — kill-on-cancel: the request token
  races the child; on cancellation the owned child is SIGKILLed by exact
  pid (Phase 0 probe pattern), pipes drained, `kill_on_drop(true)` kept as
  a safety net (plan §21: no broad process killing).
- `src/runtime/tasks.rs` — `OperationManager`: spawns one task per effect,
  maps backend errors into modal-ready failures (exit code, sanitized
  detail, retry intent), suppresses cancelled outcomes entirely, feeds
  `OperationResult`s back through the unbounded result channel.
- `src/main.rs` — startup and event loop launch effects through the manager
  with tokens pulled from the registry.
- `src/ui/components/spinner.rs` — braille frames driven by the reducer tick
  counter (deterministic for snapshots); the status bar shows frame +
  operation summary whenever foreground work is in flight.
- `src/ui/components/error_modal.rs` — centered bordered modal: operation
  summary title, exit-code row, ambiguity warning row (plan §12: duplicate-
  send warning slot), scrollable sanitized detail (wrap via `ui::text::wrap`
  with hard-chunked long URLs), `[ Retry ]`/`[ Dismiss ]` buttons with
  accent focus, key hints; layout math shared with the reducer's scroll
  clamp. Renders even in too-small terminals so failures stay recoverable.
- `src/domain/send.rs` — `SendOutcome` (Sent / SentButCopyFailed /
  FailedBeforeDelivery / Unknown) with `is_ambiguous`, classified per
  `fixtures/himalaya/send-outcomes.md`; the composer phases wire it in.
- `src/ui/text.rs` — width-aware `wrap` (Unicode-safe, never panics).
- `src/domain` — `MailboxId`/`PageRequest` gained serde derives so retry
  intents are serializable.
- `tests/fake_himalaya.rs` — `slow` modes (30 s hang) for cancellation.
- `tests/backend_contract.rs` — 20 tests (18 Phase 2 updated to
  `RequestContext` + 2 kill-on-cancel tests).
- `tests/terminal_snapshots.rs` — 11 render tests incl. spinner, modal
  contents, modal scrolling, ambiguity warning.
- `target/probe/smoke_phase3.py` — live pty smoke (scratch, uncommitted
  environment): slow-shim + real-CLI runs described below.

## Acceptance criteria (plan §19 Phase 3)

- [x] Slow fake operation does not block rendering/input — reducer test
      `input_and_ticks_keep_working_while_an_operation_is_in_flight`;
      manager test `slow_operations_do_not_block_following_work` (fast
      result lands mid-slow-op); snapshot `spinner_shows_foreground_work…`
      (list still drawn); live pty: spinner animated and keys processed
      while the slow shim ran
- [x] `Esc` kills foreground work and returns to the prior stable state —
      reducer `esc_cancels_foreground_work_and_returns_to_stable_state`
      (token fired, registry empty, old page intact, no quit) +
      `esc_during_startup_load_cancels_then_second_esc_quits`; contract
      `cancel_terminates_the_child_and_reports_cancelled` (sleep-30 fake
      killed in <5 s, `BackendError::Cancelled`); live pty: Esc mid-shim →
      "cancelled" status, page-1 rows still drawn, terminal restored on quit
- [x] A slower old mailbox request cannot replace a newer mailbox result —
      `rapid_page_next_supersedes_the_older_request` (older op cancelled +
      removed before its result arrives), `page_result_for_other_mailbox…`
      (raced mailbox switch dropped), `unknown_operation_result_is_ignored`,
      registry unit tests for supersede/cancel semantics
- [x] Retry replays equivalent typed intent with a new operation ID —
      `retry_replays_equivalent_intent_with_new_operation_id`,
      `mailboxes_failure_opens_modal_and_retry_reloads` (new id, same
      `OperationKind`, modal closed, loading state reset)
- [x] Modal detail scrolls and contains no fixture secrets —
      `modal_scroll_clamps_to_content` (reducer clamp via shared layout
      math), snapshots `error_modal_detail_scrolls` (bottom/top visible at
      clamped scrolls) and `error_modal_renders_summary_code_buttons_and…`
      (`password/token` fixture secrets absent, `█` marks present); manager
      test `failures_carry_code_retry_and_sanitized_detail`; sanitize unit
      tests cover labels, JSON shapes, Authorization/Bearer, URL userinfo,
      and pass-through of non-secret text

## Commands run / results

- `cargo fmt --all` — clean
- `cargo check --all-targets --all-features` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-features` — 157 passed, 0 failed
  (124 unit incl. registry/reducer/sanitize/modal/tasks + 20 contract +
  2 fake-himalaya + 11 render snapshots)
- Real-CLI pty smoke (himalaya 2.1.0, probe maildir, `page_size = 2`,
  Python pty driver at 152×40, `target/probe/smoke_phase3.py`):
  - happy path with a sleep-shim on PATH (1.2 s per call): startup renders
    INBOX page 1; Right → status spinner `⠋… Loading messages` while rows
    stay drawn; Esc mid-flight → `Loading messages — cancelled`, page-1
    rows unchanged; Down still moves; Esc quits with a single
    `?1049h`/`?1049l` pair
  - failure path (real CLI, nonexistent account): startup load fails →
    modal "Loading mailboxes failed", `himalaya exited with code 1`,
    sanitized detail, `[ Retry ]`/`[ Dismiss ]`; Esc dismisses and restores
    focus; second Esc exits with terminal restore

## Notes for Phase 4+

- The modal consumes all input while open; composer/reader overlays extend
  `Overlay` and the modal interception block, not the global key map.
- `OperationFailure.ambiguous` is always `false` until send lands (Phase 7
  maps `SendOutcome::is_ambiguous`); the modal already renders the warning.
- Automatic refresh stays Phase 9: the registry's `foreground` model will
  need a background flag for refresh ops then (plan §11).
- Cancellation suppresses results in the manager *and* the reducer rejects
  unknown ids — belt and suspenders; future phases get both for free.
- The tick redraw always writes ratatui's frame-reset bytes (~25 B/tick);
  harmless, but a "draw only on change" pass could silence it later.
- Maildir probe re-seeded (6 messages) before the smoke run.
