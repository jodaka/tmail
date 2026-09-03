# Phase 8 — Attachments: checklist

Status: COMPLETE (2026-09-03)

## Scope delivered (plan §19 Phase 8, kata issue bfgf + 5 children)

- **8.1 Path-entry overlay, validation, chips, removal** — the composer
  gains the mockup `.attach-row`: one chip per attached file (name + human
  size) between the body and the action row, plus a `+ attach` control.
  Enter on `+ attach` opens the path-entry overlay (plan §15: no file
  browser in v1) — a modal single-line entry with caret editing
  (`Action::DialogEdit`, `Focus::Dialog`, rendered by
  `ui/components/attachment_dialog.rs`). Enter submits the raw path to a
  new backend operation `ReadAttachment { path }`; the backend expands
  `~` inside Post (never a shell, `domain/paths.rs::expand_tilde` with an
  injected home) and requires an existing, regular, readable file within
  25 MiB (`MAX_DRAFT_ATTACHMENT_BYTES`). Every refusal is a detailed
  `BackendError::File` (path + cause) shown inline in the dialog, so
  missing/unreadable entries are retryable in place. Validated files
  become `DraftAttachment { path, name, size }` chips (metadata only —
  no bytes in state or journal), carried in `Draft`/`DraftSnapshot`
  (serde `default` keeps pre-8.1 journals loadable), so attaching and
  removing are content edits that re-arm the autosave debounce and
  journal the list. Enter on a focused chip removes it (refocus: next
  chip, last chip, or `+ attach`); same-path entries dedupe while
  same-basename files keep insertion order. Validation results apply
  only while the dialog still shows the submitted entry — Esc or an edit
  drops the stale result.
- **8.2 Outgoing attachments in MIME** — `OutboundMessage` gained
  `attachments: Vec<OutboundAttachment>` (name + validated path; metadata
  only) via `OutboundMessage::with_attachments`; the reducer maps the
  composer's chips at send time. `serialize_outbound` reads each file and
  hands it to `mail-builder` as an attachment part in insertion order;
  media type comes from a small extension map
  (`domain/paths.rs::media_type_for`, `application/octet-stream`
  fallback), and a missing/unreadable source is a detailed, retryable
  `File` refusal — never a partial send. Acceptance: a feature-gated
  round-trip test parses the serialized wire with `mail-parser` (the
  library inside Himalaya) and asserts filename (spaces intact), media
  type, exact bytes, and size all survive. Attachment-less sends stay
  single-part; remote draft copies remain text/plain (bytes enter only
  at send time).
- **8.3 Incoming attachment metadata** — the reader's attachment chips
  (filename · MIME type · human size, mapped from the `message read` DTO
  since Phase 4, deterministic in wire order per the duplicate-filenames
  fixture) gained a cursor: Tab/Shift+Tab cycle and wrap within the
  loaded message's chips (`AppState.reader_attachment`, reset on close/
  switch/retry), and the selected chip renders with a `▸` marker and the
  accent style, so it is always obvious which file the save/open keys
  act on.
- **8.4 Safe save path and collision handling** — new backend operation
  `save_attachment(ctx, AttachmentRequest) -> PathBuf` (plan §8/§15):
  `himalaya attachment download` runs into a Post-owned private tempdir
  (argv only; the `-d` directory rides as a single entry, spaces intact),
  the decoded bytes are read from the reported row path, and the final
  write uses a collision-checked `create_new` — existing downloads are
  never silently overwritten; the walk picks `name (1).ext`,
  `name (2).ext`, … deterministically and the saver returns the path
  actually written (the UI reports that path, never the requested one).
  Destination names reduce to a single component (`../../.zshenv` saves
  as `.zshenv`; `..`/empty fall back to `attachment-<part>`), so MIME
  metadata can never traverse out of the destination. Directory
  resolution: explicit request dir → `[post.attachments].downloads_dir`
  (new config key, `~` expanded inside Post) → `$HOME/Downloads`;
  missing dirs are created, refusals are detailed `File` errors.
  Reader `d` saves the selected chip via a frozen, retryable request;
  failures open the Retry/Dismiss modal, successes record the final path
  for open-reuse and report `Saved to <path>`. `AttachmentRequest` lives
  in `domain` — the app/UI-never-reference-backend layering contract
  test stays green.
- **8.5 Open adapters** — `PathOpener` trait + `SystemOpener`:
  `open` on macOS, `xdg-open` on Linux, spawned directly by argv (one
  path argument, never a shell); the tokio-reaped child leaves no
  zombies; other platforms refuse with a typed error instead of
  guessing. `OperationKind::OpenPath` is non-cancellable (a spawned
  handler app is left alone, mirroring the send rule); failures name the
  path and stay retryable. Reader `o` opens the selected chip: a file
  saved this session opens from where it landed (no duplicate
  download); otherwise save-then-open chains the opener on the
  confirmed, possibly collision-renamed path; a failed chained save
  keeps the opener off and retries replay the whole intent. The reader
  action row advertises `Save d · Open o · Tab chip` when attachments
  exist.

## Acceptance (plan §19 Phase 8)

- [x] Spaces/special characters in paths never invoke a shell — every
      path flows as a single argv entry (`attachment download -d`, the
      opener's path argument); `~` expansion happens inside Post.
      Contract tests pin `-d "/…/My Downloads/post-dl-…"` as one entry
      and the opener recording the whole path.
- [x] Missing/unreadable files produce retryable detailed errors —
      composer validation names the path and cause inline (dialog stays
      editable); a source missing at send time is a detailed `File`
      refusal with the draft intact; save/open failures open the
      Retry/Dismiss modal with replayable typed intents.
- [x] Existing downloads are not silently overwritten — `create_new`
      collision walk (`report.pdf`, `report (1).pdf`, `report (2).pdf`),
      with the plain-name fast path falling through on races; the
      previous file's bytes are asserted intact in the contract test.
- [x] MIME round-trip preserves filename, media type, bytes, and size —
      `attachment_mime_tests::attachments_round_trip_filename_media_type_bytes_and_size`
      parses the serializer's output with mail-parser and checks all four.

## Verification

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features` (all suites green; new tests: draft
  attachment snapshot round-trip, composer cycle/dedupe/removal, dialog
  editing + stale-result drops, backend source validation (missing/
  directory/oversized/unreadable/tilde), MIME round-trip, argv exactness
  with fake himalaya (ok/traversal/wrong-row/no-path/error-json modes),
  collision walk + traversal reduction + name fallbacks, downloads-dir
  resolution, reader cursor cycling/wrapping/resets, save/open reducer
  flows including retry and chain, opener doubles in the task manager.)

## Notes for Phase 9+

- Backend error details in the retry modal are sanitized (`sanitize`)
  before display; paths survive intact.
- `ReadAttachment` results are dropped when the dialog input changed —
  the currency check compares the raw entry, so `~` handling stays in
  the backend where it is testable.
- `SaveAttachment`/`OpenPath` never supersede each other (saves are
  mutations; a second save lands as a collision-renamed file, never an
  overwrite). `ReadAttachment` supers only its own kind.
- While an attachment-path dialog is open, `BackendCompleted` falls
  through the modal gate so the pending validation lands; the error
  modal still swallows it — backend results arriving while an error
  modal is open are dropped until dismiss (pre-existing Phase 3
  behavior; revisit if a real scenario needs it, e.g. Phase 12.4).
- Forwarding does not yet attach the original message's files (plan §15
  scope for v1 was outgoing files + incoming save/open); the forward
  block quotes the text body only.
- Explicit save-directory entry in the reader is backend-supported
  (`AttachmentRequest.dir`) but not yet surfaced in the UI — `d` always
  uses the configured downloads dir; a path dialog for saves can reuse
  the Phase 8.1 overlay pattern if wanted.
- `tempfile` moved from dev-dependencies to dependencies (private
  download dirs are production behavior now).
