# What Post is

A terminal e-mail client (Rust 1.95, edition 2024, ratatui) that delegates all mail
protocol work to the `himalaya` CLI (2.1.0, +smtp +imap +jmap +gmail +msgraph +maildir).
Post owns UI, state, drafts, and safety logic; Himalaya owns protocols, credentials,
MIME parsing, and delivery.

## Architecture (as built through Phase 9)

- `src/backend/` — `MailBackend` trait + typed `BackendError`; `himalaya/` adapter:
  argv-only `tokio::process::Command` (no shell), kill-on-drop + explicit SIGKILL on
  cancel, exit-status-authoritative JSON decoding, private DTOs (`dto.rs`) mapped to
  domain types in `map.rs`. Himalaya types never leave `backend/himalaya/` (enforced
  by a layering test).
- `src/domain/` — addresses, mailboxes, messages, pages, drafts, send/reply models,
  paths; serializable where they back typed retry intents.
- `src/app/` — I/O-free reducer over `Action`s returning `Vec<Effect>`;
  `OperationRegistry` (ids, typed intents, serializable `RetrySpec`s, same-kind
  supersede, background/foreground origin); overlays (error, confirm-discard,
  attachment dialog); secret sanitization before anything reaches logs or UI;
  composer/draft state machines.
- `src/runtime/` — raw-mode terminal with panic-safe restore, event stream + 250 ms
  ticks, file-only `tracing` log (daily rolling, non-blocking, `RUST_LOG`-filterable).
- `src/ui/` — theme tokens (no literal colors outside the theme), layout modes
  (full ≥120×24, compact ≥90×20, too-small otherwise), screens (mailbox, reader,
  composer, search), rich-text model, viewport math shared between renderer and
  reducer, Unicode-safe width-aware truncation/wrapping.
- Testing strategy: a fake `himalaya` executable with exact argv/stdin assertions
  (contract tests, `slow` modes for cancellation), an `.eml` corpus under the
  `test-fixtures` feature (production builds never link MIME parsing), terminal
  snapshots, reducer unit tests, pty smoke drivers (scratch, uncommitted).

## ADR 0001 — Himalaya CLI as the exclusive mail backend (2026-09-02)

**Decision.** Post v1 shells out to the installed `himalaya` binary only, through one
`MailBackend` trait. No protocol code, no Pimalaya library dependency (revisit only
with measured evidence and explicit user approval). Post writes its own `[post]`
section into the same TOML file himalaya reads (verified tolerated — himalaya ignores
unknown root tables). Errors are typed with sanitized details. A fake `himalaya` +
argv assertions is the contract-test strategy; a startup `himalaya --version` check
mitigates version drift.

**Verified constraints (himalaya 2.1.0):**

- Pass `--json` on **every** command, including mutations — without it, `flag
  {add,remove}` and `message {move,delete}` print human text with exit 0 (Phase 4
  live-smoke correction; the smoke first misclassified a *successful* flag change as
  a failure).
- JSON output has no trailing newline; errors are JSON on **stdout** with exit 1;
  exit status is authoritative.
- Envelope JSON has **no snippet**; `total`/`unread` are `null` on maildir — the UI
  must degrade (no totals, no "of N" ranges).
- Envelope ordering is date-descending, ties unordered; tests must not depend on tie
  order.
- Maildir ids are file names and **change when a message moves**; only the RFC
  `Message-ID` header is stable. Selections re-resolve by `Message-ID` after any
  move; new trash ids must be rediscovered by Message-ID.
- `message delete` is trash-first (`moved-to-trash`, then `deleted` on the trash
  alias); flag commands echo affected flags, not resulting state (authoritative flag
  state comes from `envelope list` / `message read`).
- All shared commands accept `-m <mailbox>` and fall back to the inbox alias when
  omitted — Post always passes the mailbox explicitly.
- Pagination is 1-based (`-p`, `-s`); Post maps its 0-based `PageRequest{offset,
  limit}` to `p = offset/limit + 1`, `s = limit`. Out-of-range pages exit 0 with an
  empty `envelopes` array (valid empty page).
- `message read --json` is a verbose but complete `mail_parser` serde dump; header
  names use underscores (`content_type`, `message_id`) — the mapper normalizes
  `-`/`_` on both sides.
- Reply/forward CLI templates exist but Post seeds replies itself (production builds
  never parse MIME; `mail-parser` is a test-only dependency).
- Send outcomes (`fixtures/himalaya/send-outcomes.md`): exit 0 ⇒ `Sent` (even with
  odd output — claiming failure on exit 0 invites duplicates); non-zero with
  pre-DATA markers (connect/DNS/TLS/auth/`--save`) ⇒ `FailedBeforeDelivery`
  (retry-safe); post-DATA EOF/unclassifiable ⇒ conservative `Unknown` (ambiguous).
  `SentButCopyFailed` is pre-validated away by himalaya.

## ADR 0002 — Draft storage and replacement strategy (2026-09-02)

**Findings.** Himalaya has no in-place draft update (add/copy/move/delete only);
delete is trash-first and the new trash id is not returned; only `Message-ID`
survives moves; the Gmail-specific drafts API is provider-specific and forbidden.

**Decision.**

1. Crash-safe Post-owned local journal (`<local-id>.json` + `.eml`, temp file +
   fsync + atomic rename, versioned from day one). Every revision is journaled
   **before** any remote call.
2. Remote saves coalesce: only the newest revision is ever pushed; a save of
   revision N can never mark N+1 clean; edits during a save force another save.
3. Replacement is add-then-delete: `message add -m Drafts --flag draft` (library-
   built RFC 5322 via `mail-builder`, stable `Message-ID`, Post-owned
   `X-Post-Draft-Id`), confirm the new id, only then delete the old remote copy.
4. Old-draft deletion is two-phase and best-effort: delete → locate by Message-ID in
   trash → delete again. Phase-2 failure leaves a copy in trash — acceptable, never
   data loss, reconciled opportunistically (every save sweeps Drafts for stray
   copies of the draft's Message-ID).
5. The draft's `Message-ID` is stable across revisions; a startup gap between
   recorded and remote-confirmed revisions restores the draft DIRTY so autosave
   self-heals the interrupted push.

### Config

- One shared TOML file (himalaya accounts + `[post]`). Resolution: CLI arg →
  `POST_CONFIG` → `~/.config/himalaya` → `~/Library/Application Support/himalaya`;
  unreadable/malformed files fall back to defaults while `-c` is still forwarded.
- Post keys: `[post]` account/page_size; `[accounts.<account>].email` (account
  identity for draft `From` and reply-all self-exclusion);
  `[post.mail].refresh_interval_seconds` (default 60, `0` disables, negative treated
  as 0); `[post.attachments].downloads_dir`.
- `POST_DATA_DIR` overrides the draft journal root (default:
  `~/Library/Application Support/post/drafts` on macOS, `~/.local/share/post/drafts`
  elsewhere).

### Operations / state

- Same-kind supersede: mailbox loads globally; page loads per mailbox; message
  loads per mailbox; flag toggles per message; draft saves per draft. **Never
  supersede:** moves (a lost archive is unrecoverable), sends, `SaveAttachment`,
  `OpenPath`. `ReadAttachment` supersedes only its own kind.
- Results apply only while the operation is still registered and targets the active
  context; unknown/cancelled/superseded results are dropped in both the manager and
  the reducer (belt and suspenders).
- Sends and `OpenPath` are never Esc-cancellable (killing himalaya mid-DATA could
  hide a delivered message; a spawned handler app is left alone).
- Retries replay the typed intent under a new operation id; retries of background
  operations become foreground (user-initiated) and open the modal on failure —
  intentional.
- Backend results arriving while an error modal is open are dropped until dismiss
  (pre-Phase-3 behavior; revisit if a real scenario needs it, e.g. Phase 12.4).
  Error-modal details are sanitized but keep paths intact.
- Renders redraw on every event including ticks; each tick writes ratatui's
  frame-reset bytes (~25 B/tick) — a "draw only on change" pass could silence this
  if it ever matters.

### Composer / drafts

- Printable characters (including `/` and single letters) are always composed text
  while the composer is open; Delete edits instead of trashing; global Ctrl+C/Ctrl+R
  still work; Ctrl+Enter is reserved for send.
- Esc in the composer overrides the global cancel step: it saves & leaves (no
  debounce, never a silent discard); the draft is preserved for reopening.
- Never-saved drafts snapshot as `local-unsaved`; `delete_draft` tolerates that
  (no journal entry, nothing to sweep).
- Autosave reconciliation sweeps use a single 100-envelope page of the Drafts
  mailbox (v1 assumption: few drafts) — revisit if large draft folders appear.
- `mail-builder` RFC 2047-encodes non-ASCII subjects (base64 form); maildir/IMAP
  round-trips verified live.

### Send / reply / forward

- While a send runs the composer freezes (bytes on the wire stay what the user
  saw); the send survives leaving the composer; success removes the draft wherever
  the user is (best-effort two-phase sweep; a cleanup failure is logged, never
  claimed as a send failure, leaving a copy in Drafts until manual discard).
- Sent-copy storage is himalaya's concern (no `--save` flag passed);
  `SentButCopyFailed` is classified but not currently producible through the shared
  CLI with maildir.
- Reply/forward act only on a loaded reader message (inert from the list); replies
  quote `text/plain` (HTML-only messages seed a placeholder); forwards quote the
  text body only — original attachments are not forwarded (v1 scope, plan §15).
- `OutboundMessage.message_id` accepts either bracket form; the backend strips
  brackets for mail-builder (draft-derived sends reuse the draft's stable identity).

### Attachments

- Outgoing chips are metadata only (path/name/size, journal-registered); bytes are
  read and attached at send time; media type from a small extension map
  (`application/octet-stream` fallback); a source missing at send time is a
  detailed retryable `File` refusal — never a partial send. Same-path chips dedupe;
  same-basename files keep insertion order.
- Reader chips are metadata (filename · MIME · human size, deterministic wire
  order); Tab cycles the cursor; `d` saves via a frozen retryable request, `o`
  opens (save-then-open chain on the confirmed, possibly collision-renamed path;
  same-session saves are reused without a duplicate download).
- Downloads dir resolution: explicit request dir → `[post.attachments].downloads_dir`
  → `$HOME/Downloads`; missing dirs are created. Destination names reduce to a
  single component (traversal-proof); existing downloads are never silently
  overwritten (collision walk `name (1).ext`, …); the UI reports the path actually
  written, never the requested one.
- Backend supports an explicit save directory per request (`AttachmentRequest.dir`)
  but the UI does not surface it yet — `d` always uses the configured downloads dir.

### Reader / rendering

- Opening an unread message fills missing list snippets and starts a separate
  retryable `SetRead(true)` (one typed intent per operation, not `message read
  --seen`).
- Sidebar unread counts are not recomputed after flag changes; they refresh on the
  next mailbox listing (manual refresh / auto-refresh timer).
- html2text inserts single spaces between CJK ideographs (its word-break model) —
  known fidelity trade-off; glyphs survive. Blockquote dim styling derives from the
  `> ` prefix; code spans win over the blockquote style.
- `content()` re-renders HTML per call (render + scroll clamp) — keypress-driven UI
  makes this cheap for v1; memoize per (message, width) if render latency shows up.
- Link targets are carried per `RichSpan`; the mouse phase (10) can consume them.
- Terminal size initializes from crossterm; a 0×0 pty degrades to the too-small
  message correctly.

### Search / refresh

- Search is **per-mailbox**: himalaya 2.1.0 has no reliable account-wide search
  (verified help text); account-wide would need one request per mailbox or a
  backend change.
- Query text passes through verbatim (no local parser, no rewriting); every
  trailing positional would be parsed as Himalaya's search DSL, so all flags
  precede the query (verified against real Gmail IMAP); empty queries are refused
  with guidance; DSL errors surface as sanitized command failures.
- Search totals are unknown → pagination degrades to next-availability; the range
  label drops the "of N" part; empty results name the query with a dim
  `(no results)` note once the request lands.
- The search field keeps its text after leaving a search (a future clear affordance
  can reuse `SearchEdit`); re-submitting edits the query in place; a new search
  supersedes the previous one (same mailbox).
- Auto-refresh stands down while a modal is open, the composer is on screen, or any
  operation is in flight; it retries next tick once they clear. It also refreshes
  while the reader is open (the underlying list updates beneath it) — add a focus
  guard if that proves noisy.
- `switch_mailbox` clears the whole route stack (explicit invariant).

## Deferred / open items

- Backend results dropped while an error modal is open — revisit if a real scenario
  needs it (e.g. Phase 12.4).
- "Draw only on change" pass to silence per-tick frame-reset bytes.
- Memoize HTML render per (message, width) if needed.
- UI for explicit attachment save-directory entry (backend already supports it).
- Search-field "clear" affordance.
- Focus guard for auto-refresh under the reader, if noisy.
- Account-wide search (per-mailbox fan-out or backend change).
- Live-CLI rendering smoke against the seeded maildir at Phase 12.
