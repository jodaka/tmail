# Phase 7 — MIME construction, send, reply, reply-all, forward: checklist

Status: COMPLETE (2026-09-03)

## Scope delivered (plan §19 Phase 7, kata issue 6wr9 + 7 children)

- **7.1 Library serialization** — `domain/send.rs`: `OutboundMessage` +
  `OutgoingContent` (serializable for typed retry intents, plan §12) built
  only through `OutboundMessage::from_fields`, which validates the raw
  composer address fields and refuses invalid entries (raw text preserved)
  or an empty recipient list with typed `SendBlocker`s surfaced on the
  status line — invalid addresses never reach serialization. Backend:
  `HimalayaCliBackend::serialize_outbound` builds single-part
  `text/plain` RFC 5322 with `mail-builder` (plan §14: never
  hand-concatenate MIME): account-identity `From`, library-encoded
  addresses/subject, CRLF wire format, `Message-ID` reusing the draft's
  stable identity when the send derives from a draft (ADR 0002 §D.6) and
  minted (`<stamp.send@post.local>`) otherwise. `Address` gained
  serde + `to_field()` composer round-trip helpers (quoted names,
  unrepresentable names degrade to the bare address).
- **7.2 Send through the Himalaya stdin contract** —
  `MailBackend::send_message(ctx, OutboundMessage) -> BackendResult<SendOutcome>`
  pipes the serialized bytes to `himalaya message send --json` stdin via
  `process::run_with_stdin` (tokio Command, argv arrays only,
  kill-on-cancel). `classify_send` implements the Phase 0 characterization
  (`fixtures/himalaya/send-outcomes.md`): exit 0 → `Sent` (even with odd
  output — claiming failure on exit 0 would invite duplicates); non-zero
  with pre-DATA markers (connect/DNS/TLS/auth/`--save` target) →
  `FailedBeforeDelivery`; everything else (DATA-phase EOF, unclassifiable)
  → conservative `Unknown`; cancellation stays a suppressed
  `BackendError::Cancelled`. `SendOutcome` variants now carry the exit
  `code` for the modal. Structural refusals (missing account identity, no
  recipients, spawn I/O) remain `Err` and are never ambiguous.
  `OperationKind::Send` freezes the payload (serializable retry intent),
  never supersedes, and is never Esc-cancellable (`is_cancellable`):
  killing himalaya mid-DATA would hide a possibly-delivered message.
- **7.3 Reply/forward seeding** — new `domain/reply.rs`:
  `seed_reply`/`seed_forward` are pure, fixture-tested Post logic. Post
  constructs seeds itself (plan §14 allows this with fixture tests)
  because production builds never parse MIME (ADR 0001: `mail-parser` is
  test-fixtures only), so the verified himalaya reply templates cannot be
  consumed at runtime. Reply: sender as recipient, `Re:` prefix kept
  unique, quoted body under `On <date>, <author> wrote:` (caret seeds at
  the top), threading headers extended by the original `Message-ID`.
  Forward: empty recipients, `Fwd:` subject, Gmail-style
  `---------- Forwarded message ---------` header block (From/Date/
  Subject/To + optional Cc), new thread (no reply headers). Missing data
  degrades gracefully (unknown date, no body, no Message-ID). Reducer:
  `Action::Reply`/`Forward` seed a clean draft on top of the reader
  (Esc returns to reading); an existing draft is never clobbered.
- **7.4 Reply headers** — `MessageHeaders` gained `in_reply_to`/
  `references`, mapped from `message read` dumps to bare, bracket-stripped,
  whitespace-normalized ids (new `TextList` DTO kind for the References
  serde shape). `Draft`/`DraftSnapshot` carry the headers; serde `default`
  keeps pre-7.4 journal entries loadable (ADR 0002 versioning); the
  journal restore rebuilds a crash-interrupted reply intact; sends
  serialize them unchanged.
- **7.5 Reply-all recipients** — sender + original To land in To, original
  Cc stays Cc minus anyone already present; deduplication is
  case-insensitive by email, first occurrence's display form wins; the
  configured account address (new `AppState.account_email`, fed from
  `[accounts.<account>].email`) is excluded — replying to your own mail
  never mails yourself; without a configured address nothing is excluded.
- **7.6 Confirmed send** — `send_from_composer`: Ctrl+Enter/Send gated to
  the composer route (inert anywhere else, plan §19 acceptance); refusals
  (no recipients, invalid entries, a send already in flight) surface on
  the status line and start nothing. While a send runs the composer
  freezes (`ComposerState.sending`; edits ignored, Send reads `Sending…`)
  so the bytes on the wire stay what the user saw; the draft may still be
  left with Esc (save/leave) and the send completes in the background.
  `SendOutcome::Sent`: status `Message sent`, composer dropped, prior
  route restored, and the draft resolved through the two-phase backend
  sweep (journal entry + remote copies) as a best-effort
  `OperationKind::DeleteDraft { reason: Sent }` — a cleanup failure is
  logged, never claimed as a send failure (ADR 0002 consequences). Hard
  refusals and `FailedBeforeDelivery` keep the draft intact and editable,
  opening Retry/Dismiss.
- **7.7 Ambiguous outcomes** — `SentButCopyFailed`/`Unknown` open the
  Retry/Dismiss modal with the ambiguity flag; the modal warns
  "the outcome is unclear — retrying may send a duplicate" and its title
  reads `<summary> — outcome unclear`, never claiming definite failure
  (plan §12). Status distinguishes `Send outcome unclear` from
  `Send failed` (the `FailedBeforeDelivery` case, retry-safe, not
  ambiguous); neither ambiguous state ever reports `Message sent`. Retry
  replays the frozen `OutboundMessage` verbatim under a new operation id
  and re-freezes the composer while the replay runs.

## Acceptance criteria (plan §19 Phase 7)

- [x] Integration fixtures parse sent output back into expected
      headers/body — `sent_output_parses_back_into_expected_headers_and_body`
      and `sent_reply_headers_parse_back_into_the_wire_format` pipe the
      exact bytes through a fake himalaya and parse them with
      `mail-parser` (the library inside Himalaya) via the production
      `parse_raw_message` fixture path; plus the live wire-payload check
      below
- [x] Reply and reply-all recipient tests cover duplicates and
      self-addresses — `reply_all_deduplicates_case_insensitively_…`,
      `reply_all_excludes_the_configured_own_address`,
      `reply_all_to_your_own_message_…`, reducer
      `reply_all_merges_recipients_dedups_and_excludes_self`
- [x] Forward representation is fixture-tested —
      `forward_seed_from_real_plain_fixture_carries_a_clear_block` (real
      `message read` probe output) and
      `forward_of_a_real_himalaya_message_has_a_header_block`
- [x] Ctrl+Enter sends only from composer — `ctrl_enter_sends_only_from_the_composer`
- [x] Failed send keeps the draft intact — `send_failure_keeps_the_draft_intact`,
      `failed_before_delivery_send_reports_definite_failure_safely`,
      `ambiguous_send_opens_the_duplicate_warning_and_keeps_the_draft`
- [x] Ambiguous send never claims definite failure or success —
      `ambiguous_send_never_shows_a_failure_title_or_status`, ambiguous
      modal title "— outcome unclear"

## Commands run / results

- `cargo fmt --all` — clean
- `cargo check --all-targets --all-features` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-features` — 377 passed, 0 failed
  (lib incl. send/reply/reducer + 38 backend contract + 2 reply-seed
  fixture + 16 rich + 25 terminal snapshots)

## Live verification (real himalaya 2.1.0 + probe maildir + SMTP sink)

Driven through the real `HimalayaCliBackend` (scratch binary, probe env
restored afterwards):

- `list_mailboxes` resolves Inbox/Drafts/Sent/Archive roles from the real
  maildir listing.
- `save_draft` → real maildir id returned; journal recorded first.
- `send_message` → `SendOutcome::Sent`; the sink captured the wire bytes,
  which carry exactly the expected RFC 5322: stable draft `Message-ID`
  (`<live-1.draft@post.local>`), `In-Reply-To`/`References` preserved,
  quoted-printable body with a non-ASCII em-dash intact.
- Post-send cleanup (the `DeleteDraft(Sent)` path) removed the journal
  entry and the remote draft copy; the Drafts mailbox is empty again.
- Dead SMTP port → `FailedBeforeDelivery` (nothing transmitted, retry
  safe).
- Silent-drop sink (payload transmitted, connection closed before the
  final 250) → `Unknown`, flagged ambiguous — the Phase 0 ambiguous
  scenario reproduces through the production path.

## Notes for Phase 8+

- Sends are not user-cancellable by design (Esc falls through to
  navigation while one is foregrounded); the frozen `OutboundMessage`
  makes retries exact.
- The composer freezes during a send; a send in flight survives leaving
  the composer, and the result applies whenever it lands (success takes
  the draft away wherever it is).
- Sent-copy storage is himalaya's concern (no `--save` flag is passed);
  `SentButCopyFailed` is classified but not currently producible through
  the shared CLI with maildir (`fixtures/himalaya/send-outcomes.md`).
- Forwarded attachments are not included yet — attachment bytes arrive
  with Phase 8 (`attachment download`); the forward block quotes the
  text body only.
- Reply seeds quote the `text/plain` body; HTML-only messages seed
  `(no plain text body)`.
- Reply/forward from the message list is inert (full message data is
  required); the actions act on a loaded reader message.
- Post-send draft-cleanup failures are logged only (ADR 0002 best-effort);
  a failed sweep leaves the draft copy in Drafts until a manual discard.
- `OutboundMessage.message_id` accepts either bracket form; the backend
  strips brackets for mail-builder (draft-derived sends reuse
  `<…@post.local>` draft identities).
