# Phase 6 — Built-in composer and safe draft autosave: checklist

Status: COMPLETE (2026-09-02)

## Scope delivered (plan §19 Phase 6, kata issue 1a7p + 7 children)

- **6.1 Field focus + body editing** — `ratatui-textarea 0.9.2` (shares
  ratatui-core 0.1.2 with our ratatui 0.30.2, no dependency duplication) as
  the body editor (plan §5). New `src/app/composer.rs`: `ComposerState`
  (focus, caret, Cc/Bcc toggles, body `TextArea<'static>`, `Draft`) and the
  `ComposerField` cycle To → CcToggle/Cc → BccToggle/Bcc → Subject → Body →
  Send → Discard, driven by Tab/Shift+Tab and Up/Down (the caret crosses
  into the neighbouring field at the body's top/bottom edge). New
  `ComposerEdit` action (Char/Backspace/Delete/Newline + caret moves); the
  reducer routes edits to the focused control. Keyboard: printable
  characters (including `/` and letters) are always composed text in the
  composer — single-letter shortcuts cannot fire (plan §10); Delete edits
  instead of trashing; global Ctrl+C/Ctrl+R still work. New
  `src/ui/screens/composer.rs` per mockup `new-mail.html` (8-column
  right-aligned labels, hairline rules, inline reversed-cell caret — the
  terminal cursor stays hidden app-wide, right-aligned Cc/Bcc toggles that
  hide while their field is revealed, Send/Discard action row) plus a
  COMPOSE status-bar branch. `Route::Composer` + `AppState.composer` keep
  the draft when the route pops, so compose reopens it.
- **6.2 Address parsing/validation** — `domain/address.rs`: `Address::parse_entry`
  (bare / `Name <email>` / quoted names; rejects unbalanced brackets,
  stray angle brackets in names, whitespace, undotted domains),
  `is_valid_email` (exactly one `@`, non-empty local, non-empty dotted
  domain — documented strictness), `address_entries` (comma/semicolon
  tokenization with byte ranges, empty entries skipped),
  `parse_address_list` → `Vec<Result<Address, String>>` for Phase 7 send
  gating. The composer flags invalid entries live in the warning color,
  valid ones in text color; invalid entries never reach the wire.
- **6.3 Draft model + 2 s debounce + save operation** — `domain/draft.rs`:
  `Draft` (fields, `revision`/`saved_revision`, `saved_at`,
  `last_edit_at`, save state, stable `DraftId` + RFC `Message-ID` minted
  once before the first save) and the plan's exact state machine. Edits
  bump the revision and re-arm the debounce; caret moves do not.
  `Action::Tick { now }` injects the wall clock (`AppState.clock`) — the
  reducer stays deterministic (plan §20). Backend side:
  `MailBackend::save_draft` = journal record BEFORE any remote call →
  library-built RFC 5322 (`mail-builder`; stable `Message-ID`, Post-owned
  `X-Post-Draft-Id`, valid parsed addresses only) → `message add -m
  Drafts --flag draft --json` (stdin piping added to `process.rs`) →
  delete the old remote copy only after the new id is confirmed →
  `journal.mark_remote`. `src/backend/journal.rs`: Post-owned crash-safe
  journal (ADR 0002 §D.1) — versioned entries, temp file + fsync +
  atomic rename, remote-confirmation bookkeeping, corrupt-file-safe
  load, path-traversal-proof ids. `OperationKind::SaveDraft` supersedes
  same-draft saves so only the newest revision is ever pushed; snapshots
  are serializable (plan §12 retry intents).
- **6.4 Remote strategy completion** — startup restore: `Action::LoadDrafts`
  (once at startup) reads the journal purely locally; the newest revision
  is rebuilt into the composer (`ComposerState::from_draft`) without
  clobbering live editing; a gap between recorded and remote-confirmed
  revisions restores the draft DIRTY so the autosave self-heals the
  interrupted push. Two-phase old-copy deletion (ADR 0002 §D.4): after a
  trash-first delete, locate the moved copy in trash by Message-ID and
  delete again — detached, best-effort, failures logged. Stray
  reconciliation on save (§D.5): every save sweeps Drafts for copies
  matching the draft's Message-ID (excluding the confirmed new id) and
  removes them two-phase. `MailBackend::delete_draft` (journal first,
  then remote sweep) prepared for the discard flow. Account identity
  (`email`, `display-name`) parsed from config for the drafts' `From`.
- **6.5 Force save on leave** — Esc in the composer overrides the global
  cancel step: it leaves (route pops, MessageList focus) and forces a
  save of the dirty draft — no debounce, never a silent discard. A save
  of the exact current revision already in flight is neither cancelled
  nor duplicated (`OperationRegistry::is_saving_draft`); an in-flight
  older save is superseded by the forced one. A draft remains fully
  preserved for reopening. Debounce-stall fix: an edit recorded before
  the first tick arms its window at the next tick.
- **6.6 Confirmed discard** — `Overlay::ConfirmDiscard(DiscardDialog)` +
  `src/ui/components/confirm_modal.rs` (centered bordered dialog, subject
  preview, "deleted permanently" warning, [ Discard ] / [ Keep editing ]
  with Keep as the safe default). Modal handling generalized in the
  reducer (`modal_reduce` dispatches per overlay). Confirm: local draft
  removed immediately (composer cleared, route popped), in-flight saves
  of the same draft cancelled (`OperationRegistry::cancel_draft_saves`,
  so a late result can never resurrect the draft), and one `DeleteDraft`
  operation removes the journal entry and sweeps remote copies two-phase.
  A failed remote sweep opens the Retry/Dismiss modal without rolling
  back the discard.
- **6.7 Autosave status line** — header status per mockup `.draft-status`:
  `Unsaved changes` (debouncing), `Saving…` (operation in flight),
  `Draft saved · HH:MM` (stamped from the injected clock at
  confirmation), `Save failed` (warning color, persists after the modal
  is dismissed, content retained). A never-edited draft shows nothing.

## Acceptance criteria (plan §19 Phase 6)

- [x] Enter inserts newline; Ctrl+Enter is reserved for send —
      `enter_inserts_newline_only_in_body`, keyboard tests (Ctrl+Enter →
      `Action::Send`, a Phase 7 placeholder), composer caret tests
- [x] Esc leaves without discarding — `esc_leaves_the_composer_…`,
      `compose_again_reopens_the_preserved_draft`, `esc_mid_debounce_…`
- [x] Saving revision N cannot mark later revision N+1 clean —
      `saving_revision_n_cannot_mark_revision_n_plus_1_clean`,
      `out_of_order_confirmations_never_lower_saved_revision`
- [x] Edits during a save trigger another save —
      `edits_during_a_save_force_another_save` (model), the chained
      follow-up in the reducer, `esc_with_newer_edits_supersedes_…`
- [x] Crash/restart preserves the last safe draft under the selected
      strategy — journal record before any remote call (contract-proven
      ordering), startup restore + self-heal tests, live two-phase
      replacement verified against real himalaya
- [x] Autosave failure opens Retry/Dismiss and retains content —
      `draft_save_failure_opens_retry_modal_and_retains_content`,
      `dismiss_after_failure_keeps_the_draft_…`, `Save failed` status

## Commands run / results

- `cargo fmt --all` — clean
- `cargo check --all-targets --all-features` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-features` — 323 passed, 0 failed
  (unit incl. composer/draft/journal/reducer + 32 backend contract +
  2 fake-himalaya + 16 rich + 25 terminal snapshots)

## Live verification (real himalaya 2.1.0 + probe maildir)

- Two successive `save_draft` calls: exactly ONE remote draft remained in
  Drafts (add-then-delete replacement), stable `Message-ID` preserved,
  `\Draft` flag set, `X-Post-Draft-Id` written, journal confirmed
  revision 2; probe environment restored afterwards.
- Two-phase delete wire shapes re-verified: `moved-to-trash` → located by
  Message-ID in trash → `deleted`.

## Notes for Phase 7+

- `Action::Send` is wired (Ctrl+Enter and the Send button route to it)
  and currently logs as unimplemented: Phase 7 serializes + sends.
- `parse_address_list` returns per-entry `Result`s — Phase 7 send should
  refuse to send while any entry is invalid (the composer only flags).
- `Domain::Draft::snapshot()` falls back to `local-unsaved` for
  never-saved drafts; `delete_draft` tolerates it (no journal entry, no
  remote copies, nothing to sweep).
- Draft reconciliation sweeps use a single 100-envelope page of the
  Drafts mailbox (v1: few drafts). Revisit if large draft folders appear.
- mail-builder RFC 2047-encodes non-ASCII subjects (base64 form);
  maildir/IMAP round-trips were verified live.
- The statusbar advertises "esc save & leave" — implemented in 6.5.
- `POST_DATA_DIR` overrides the journal root (default:
  `~/Library/Application Support/post/drafts` on macOS,
  `~/.local/share/post/drafts` elsewhere).
