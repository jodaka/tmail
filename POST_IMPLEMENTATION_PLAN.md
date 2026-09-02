# Post v1 implementation plan

> Execution brief for Codex or another implementation agent. Treat this document as the product and engineering specification for v1. Implement it incrementally; do not silently expand scope. When a backend assumption is uncertain, prove it with a small executable spike before building dependent UI.

## 1. Product summary

Build **Post**, a Gmail-inspired, keyboard-first terminal email client in Rust using Ratatui. Post delegates accounts, credentials, protocols, provider behavior, mailbox semantics, search, and delivery to the Himalaya CLI. Post owns interaction, state, presentation, message rendering, composition, and orchestration.

The supported provider rule is simple:

```text
Post supports the capabilities exposed by Himalaya's shared CLI.
Post contains no provider-specific protocol or Gmail API code.
```

The three HTML files supplied with this plan are visual references:

- `tui-mail-mockup.html`: mailbox/message-list layout
- `viewer.html`: reader layout; treat it as a **single-message** reader in v1
- `new-mail.html`: composer layout

Mockups are visual direction, not literal behavior. Section 4 lists the intentional overrides.

## 2. Non-negotiable product principles

1. Gmail-inspired visual hierarchy and shortcuts, without Gmail-specific backend logic.
2. Keyboard-first; arrow keys always work and no Vim knowledge is assumed.
3. The UI never blocks on a mail operation.
4. Himalaya owns mail protocols and account credentials; Post owns user experience.
5. Detailed, recoverable failures: errors appear in a scrollable Retry/Dismiss modal.
6. Deterministic rendering; no AI is used to render email.
7. macOS is required, Linux is supported, and Windows is out of scope for v1.

## 3. Frozen v1 scope

### Included

- One configured account at a time
- Discover and browse mailboxes exposed by Himalaya
- Explicitly paginated message lists
- Open and read one message at a time
- Plain-text and high-quality HTML-to-terminal rendering
- Compose and send
- Reply, reply-all, and forward
- Drafts with debounced autosave in the built-in editor
- Optional external `$EDITOR` configured in TOML
- Incoming and outgoing attachments
- Backend search using the query string unchanged
- Archive and trash/delete
- Mark read/unread
- Star/unstar
- Configurable periodic refresh, default 60 seconds
- Manual refresh
- Cancellation of pending work with `Esc`
- Detailed Retry/Dismiss error modal
- Keyboard navigation and basic mouse support
- Responsive dark UI based on the mockups
- One user-facing TOML configuration file
- Session-only in-memory data caching

### Explicitly out of scope

- Threads/conversation aggregation
- Multiple active accounts or account switcher
- Labels/mailbox management UI
- Settings UI
- Help screen
- Vim mode or `j`/`k` navigation
- Persistent offline mail cache or full offline mode
- AI-based rendering or summarization
- Remote images or tracking pixels
- Snooze, schedule-send, storage quota display
- Sophisticated terminal file browser
- Windows support
- SSH/tmux-specific integration
- Provider-specific APIs or behaviors in Post

## 4. Mockup interpretation and behavioral overrides

Preserve the mockups' spacing, density, hierarchy, sidebar, top search, message rows, status bar, reader actions, attachment chips, composer fields, and dark visual style.

Apply these v1 overrides:

| Mockup element | v1 behavior |
| --- | --- |
| Reader contains three thread messages | Render exactly one selected message |
| `j`/`k` shortcuts | Use Up/Down; do not implement `j`/`k` |
| `?` help | Remove; no help UI in v1 |
| Labels section | Do not render label-management UI |
| Snoozed, Scheduled, storage quota | Do not render unless a generic Himalaya mailbox happens to have the same name; no special behavior |
| Gmail-named folders | Populate the sidebar from Himalaya mailboxes and known mailbox aliases |
| Enter sends from composer | `Enter` inserts a newline; `Ctrl+Enter` sends |
| Esc discards draft | `Esc` saves/leaves the draft; discard is an explicit destructive action |
| Thread position/count | Omit |
| Sender-provided colors/styles | Map semantic HTML to Post theme tokens |

## 5. Technical direction

### Stack

- Stable Rust, Rust 2024 edition
- Ratatui + Crossterm
- Tokio for the event loop, timers, and child processes
- `serde`, `serde_json`, and TOML support
- `ratatui-textarea` for the built-in body editor
- `html2text` as the initial HTML rendering engine
- A robust MIME parser such as `mail-parser`
- A MIME/RFC 5322 builder such as `mail-builder`
- `tracing` and a file-based subscriber
- `thiserror`/`anyhow` according to layer: typed library errors, contextual application errors

Do not hard-code dependency versions from this document. Select mutually compatible current releases, commit `Cargo.lock`, and record any version-driven API decisions in the repository.

### Architecture

```text
Crossterm events / timers / task results
                 |
                 v
          Action translation
                 |
                 v
       Reducer + application state
          |                |
          | render         | effects
          v                v
       Ratatui UI     Operation manager
                            |
                            v
                     MailBackend trait
                            |
                            v
                  HimalayaCliBackend
                            |
                            v
                 Himalaya subprocesses
```

Rules:

- UI components render state and emit actions; they do not invoke Himalaya.
- Reducers are deterministic and do not perform I/O.
- Effects launch typed backend requests.
- Every request and result carries an `OperationId`.
- Stale or cancelled results never mutate current state.
- Himalaya JSON types never escape `backend/himalaya/`.
- Invoke Himalaya with `tokio::process::Command`, never through a shell string.

## 6. Target repository structure

Start with one binary crate. Split crates only if later evidence justifies it.

```text
.
├── Cargo.toml
├── Cargo.lock
├── README.md
├── config.example.toml
├── fixtures/
│   ├── himalaya/
│   ├── mail/
│   └── snapshots/
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── app/
│   │   ├── mod.rs
│   │   ├── action.rs
│   │   ├── effect.rs
│   │   ├── focus.rs
│   │   ├── operation.rs
│   │   ├── reducer.rs
│   │   ├── route.rs
│   │   └── state.rs
│   ├── backend/
│   │   ├── mod.rs
│   │   ├── traits.rs
│   │   └── himalaya/
│   │       ├── mod.rs
│   │       ├── command.rs
│   │       ├── dto.rs
│   │       ├── map.rs
│   │       └── process.rs
│   ├── config/
│   │   ├── mod.rs
│   │   └── validation.rs
│   ├── domain/
│   │   ├── address.rs
│   │   ├── attachment.rs
│   │   ├── draft.rs
│   │   ├── mailbox.rs
│   │   ├── message.rs
│   │   └── page.rs
│   ├── editor/
│   │   ├── builtin.rs
│   │   └── external.rs
│   ├── input/
│   │   ├── keyboard.rs
│   │   └── mouse.rs
│   ├── mail/
│   │   ├── html.rs
│   │   ├── mime.rs
│   │   ├── render.rs
│   │   └── reply.rs
│   ├── runtime/
│   │   ├── events.rs
│   │   ├── refresh.rs
│   │   ├── tasks.rs
│   │   └── terminal.rs
│   └── ui/
│       ├── mod.rs
│       ├── layout.rs
│       ├── theme.rs
│       ├── components/
│       │   ├── error_modal.rs
│       │   ├── sidebar.rs
│       │   ├── spinner.rs
│       │   ├── statusbar.rs
│       │   └── topbar.rs
│       └── screens/
│           ├── composer.rs
│           ├── mailbox.rs
│           ├── reader.rs
│           └── search.rs
└── tests/
    ├── backend_contract.rs
    ├── fake_himalaya.rs
    ├── maildir_integration.rs
    └── terminal_snapshots.rs
```

If an existing repository uses a different but coherent layout, preserve it and map these responsibilities into it.

## 7. Core domain model

IDs are opaque backend strings. Do not parse meaning from them.

```rust
pub struct Mailbox {
    pub id: MailboxId,
    pub name: String,
    pub role: Option<MailboxRole>,
    pub unread_count: Option<u64>,
    pub total_count: Option<u64>,
}

pub enum MailboxRole {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Spam,
    Trash,
}

pub struct MessageSummary {
    pub id: MessageId,
    pub mailbox_id: MailboxId,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub subject: String,
    pub snippet: Option<String>,
    pub timestamp: DateTime<FixedOffset>,
    pub is_read: bool,
    pub is_starred: bool,
    pub has_attachments: bool,
}

pub struct Message {
    pub id: MessageId,
    pub mailbox_id: MailboxId,
    pub headers: MessageHeaders,
    pub plain_body: Option<String>,
    pub html_body: Option<String>,
    pub attachments: Vec<Attachment>,
    pub raw: Option<Vec<u8>>,
}

pub struct Draft {
    pub local_id: DraftId,
    pub remote_id: Option<MessageId>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<DraftAttachment>,
    pub reply_context: Option<ReplyContext>,
    pub revision: u64,
    pub saved_revision: Option<u64>,
}

pub struct PageRequest {
    pub mailbox_id: MailboxId,
    pub offset: usize,
    pub limit: usize,
}

pub struct Page<T> {
    pub items: Vec<T>,
    pub offset: usize,
    pub limit: usize,
    pub total: Option<usize>,
}
```

The exact date/time type may change, but preserve source timezone when possible and centralize display formatting.

## 8. Backend contract

Define an async, mockable interface around Post's needs rather than mirroring every Himalaya command:

```rust
#[async_trait]
pub trait MailBackend: Send + Sync {
    async fn capabilities(&self, ctx: RequestContext) -> Result<Capabilities>;
    async fn list_mailboxes(&self, ctx: RequestContext) -> Result<Vec<Mailbox>>;
    async fn list_messages(&self, ctx: RequestContext, page: PageRequest)
        -> Result<Page<MessageSummary>>;
    async fn search(&self, ctx: RequestContext, request: SearchRequest)
        -> Result<Page<MessageSummary>>;
    async fn get_message(&self, ctx: RequestContext, locator: MessageLocator)
        -> Result<Message>;
    async fn set_read(&self, ctx: RequestContext, locator: MessageLocator, read: bool)
        -> Result<()>;
    async fn set_starred(&self, ctx: RequestContext, locator: MessageLocator, starred: bool)
        -> Result<()>;
    async fn archive(&self, ctx: RequestContext, locator: MessageLocator) -> Result<()>;
    async fn trash(&self, ctx: RequestContext, locator: MessageLocator) -> Result<()>;
    async fn save_draft(&self, ctx: RequestContext, draft: SerializedDraft)
        -> Result<SavedDraft>;
    async fn delete_draft(&self, ctx: RequestContext, locator: MessageLocator)
        -> Result<()>;
    async fn send(&self, ctx: RequestContext, message: Vec<u8>) -> Result<SendOutcome>;
    async fn save_attachment(&self, ctx: RequestContext, request: AttachmentRequest)
        -> Result<PathBuf>;
}
```

`RequestContext` includes `OperationId`, cancellation, selected account/config, and safe tracing fields. `Capabilities` allows UI actions to be disabled when the shared backend does not expose an operation.

Map semantic operations such as archive and trash inside the backend adapter. UI code must not guess folder names.

## 9. Application state and actions

Use a route stack so returning from reader/composer restores the exact previous mailbox/search page, focus, and selection.

```rust
pub enum Route {
    Mailbox(MailboxRoute),
    Message(MessageRoute),
    Compose(ComposeRoute),
    Search(SearchRoute),
}

pub enum Overlay {
    Error(ErrorDialog),
    ConfirmDiscard(ConfirmDialog),
    AttachmentPath(AttachmentPathDialog),
}

pub struct AppState {
    pub routes: Vec<Route>,
    pub mailboxes: Loadable<Vec<Mailbox>>,
    pub operations: OperationRegistry,
    pub overlay: Option<Overlay>,
    pub status: StatusState,
    pub config: PostConfig,
    pub quit_requested: bool,
}
```

All input sources map into the same action vocabulary:

```rust
pub enum Action {
    MoveUp,
    MoveDown,
    PagePrevious,
    PageNext,
    Activate,
    BackOrCancel,
    FocusNext,
    FocusPrevious,
    OpenSearch,
    SubmitSearch,
    Compose,
    Reply,
    ReplyAll,
    Forward,
    Archive,
    Trash,
    ToggleStar,
    MarkUnread,
    Refresh,
    Send,
    LeaveComposer,
    DiscardDraft,
    RetryError,
    DismissError,
    BackendCompleted(OperationResult),
    Tick,
    Resize { width: u16, height: u16 },
    Quit,
}
```

Reducer tests must cover invalid/empty selections, stale results, cancellation, route restoration, overlay priority, resize, and refresh preserving selection.

## 10. Input contract

Default keys:

| Context | Key | Action |
| --- | --- | --- |
| Global | `Up` / `Down` | Move selection or scroll focused area |
| Global | `Left` / `Right` | Previous/next explicit page where applicable |
| Global | `Enter` | Open/activate focused control |
| Global | `Esc` | Cancel work, close overlay, or go back—in that order |
| Global | `Tab` / `Shift+Tab` | Next/previous focus |
| Global | `/` | Focus search |
| Global | `c` | Compose |
| Global | `Ctrl+R` | Manual refresh |
| List/reader | `r` | Reply |
| List/reader | `a` | Reply-all |
| List/reader | `f` | Forward |
| List/reader | `e` | Archive |
| List/reader | `s` | Star/unstar |
| List/reader | `u` | Mark unread |
| List/reader | `Delete` | Trash |
| Composer | `Enter` | Insert newline in body or activate focused non-body control |
| Composer | `Ctrl+Enter` | Send |
| Composer | `Esc` | Save/leave; never silently discard |

Single-letter shortcuts must not fire while editing a text field. No `j`/`k` aliases in v1.

Mouse support translates hit-tested regions into the same actions:

- Click mailbox, message row, search field, action button, attachment, or modal button.
- Wheel scrolls the focused list/reader/modal.
- Do not create mouse-only behavior.

## 11. Async operations, cancellation, and refresh

Every operation has:

```rust
pub struct Operation {
    pub id: OperationId,
    pub kind: OperationKind,
    pub started_at: Instant,
    pub retry: Option<RetrySpec>,
    pub cancellation: CancellationToken,
}
```

Required behavior:

- Spawn Himalaya directly and capture stdout/stderr separately.
- Pipe serialized mail to stdin when required.
- Keep a child-process handle so cancellation terminates it.
- On `Esc`, cancel the currently foregrounded cancellable operation first.
- Ignore completion for unknown, cancelled, or superseded IDs.
- Foreground work shows a spinner without freezing input.
- Automatic refresh is background work and must not steal focus.
- Default refresh interval is 60 seconds and is configurable.
- Refresh only the visible mailbox/search context needed for v1.
- Preserve selected message by `MessageId`, then rebuild its index.
- Preserve explicit page unless the page becomes invalid.
- Never interrupt composer editing for refresh.
- Suppress repeated identical automatic-refresh error modals until state changes or the user manually retries.

Terminal restoration must work after normal exit, Ctrl+C, error return, and panic.

## 12. Error model

All operational failures open a centered, scrollable modal with:

- Human-readable operation summary
- Full safe causal detail available
- Himalaya exit status
- Sanitized stderr
- Retry button
- Dismiss button
- Warning when outcome is ambiguous

Store a serializable/cloneable `RetrySpec`, not a closure. Retrying creates a new `OperationId`.

Never display or log credentials, OAuth tokens, authorization headers, secret config values, or raw command environments. Implement a small, tested sanitization layer before details reach logs or the UI.

Sending has a special ambiguous state: delivery may have succeeded while saving a Sent copy failed. Represent outcomes explicitly:

```rust
pub enum SendOutcome {
    Sent,
    SentButCopyFailed { detail: String },
    FailedBeforeDelivery { detail: String },
    Unknown { detail: String },
}
```

For `SentButCopyFailed` or `Unknown`, Retry remains available because the product requires it, but the modal must state that retry could send a duplicate.

## 13. Message rendering contract

### MIME selection

1. Parse MIME robustly; do not hand-roll MIME traversal.
2. When both HTML and plain text exist, prefer HTML for the richer Gmail-like experience.
3. Fall back to plain text if HTML is absent or rendering fails.
4. Preserve the original message body for retry/debugging without exposing it in ordinary logs.
5. Never fetch remote images in v1.

### Semantic terminal rendering

Convert HTML into an app-owned `RichText` model, then into Ratatui lines/spans. Do not let a third-party renderer's types leak into UI state.

| HTML semantic | Terminal representation |
| --- | --- |
| Heading/strong | Bold |
| Emphasis | Italic when supported; otherwise semantic fallback |
| Link | Accent + underline; retain target for selection/copy/open later |
| Blockquote | Dim text with left marker |
| Ordered/unordered list | Stable indentation and markers |
| `pre`/`code` | Preserve whitespace; horizontal scrolling or safe wrapping |
| Table | Width-aware text layout with graceful fallback |
| `hr` | Separator |
| Image | Alt text only |
| Sender color/background | Ignore; use Post theme |

Reflow at the current reader width after terminal resize. Handle Unicode safely and never slice strings at invalid byte boundaries.

Create `.eml` fixtures for plain text, multipart/alternative, malformed HTML, newsletters, receipts, GitHub notifications, nested quotes, signatures, tables, code/pre, long URLs, RTL, emoji, empty body, duplicate filenames, and large attachment metadata.

## 14. Composer and drafts

Composer fields: To, optional Cc, optional Bcc, Subject, body, attachments, autosave status, Send, and explicit Discard.

Autosave state machine:

```text
edit -> increment revision -> dirty -> debounce 2 seconds -> saving
   -> success for latest revision -> saved(timestamp)
   -> success for older revision -> remain dirty and save again
   -> failure -> error modal + unsaved state
```

Also force a save when leaving the built-in composer. Leaving returns to the prior route and preserves the draft. Discard requires a confirmation and deletes both local and remote draft state only after confirmation.

### Required draft safety spike

Before implementing the full composer, establish how the installed/supported Himalaya CLI version handles:

- Creating a draft
- Returning its stable identifier
- Updating/replacing the same draft
- Deleting the old draft
- Flags/folder semantics
- Failure between add-new and delete-old

Preferred strategy: update a stable remote draft in place.

Fallback if the CLI cannot update safely:

1. Persist the working draft in a small crash-safe local draft journal; this is draft state, not a mail cache.
2. Coalesce remote saves.
3. Add the replacement draft first.
4. Delete the old remote draft only after the new ID is confirmed.
5. Reconcile duplicate remote drafts on the next successful save/startup using Post-owned draft metadata where possible.
6. Never risk losing the last known-good draft to achieve deduplication.

Document the selected strategy in an architecture decision record before dependent implementation.

### MIME and replies

- Build outgoing RFC 5322/MIME with a library; do not manually concatenate MIME.
- Himalaya remains responsible for delivering/storing the serialized message.
- Reply preserves `Message-ID`, `In-Reply-To`, and `References` semantics.
- Reply-all deduplicates addresses and excludes the configured account's own address.
- Forward includes a clear forwarded header block and attachments according to tested Himalaya behavior.
- Prefer Himalaya-provided reply/forward templates if they are structured and reliable; otherwise construct them in Post with fixture tests.

### External editor

Implement after the built-in composer:

1. Save the draft.
2. Leave raw/alternate-screen mode.
3. Write the body to a secure temporary file.
4. Spawn the configured editor without a shell.
5. Wait for exit; no background autosave is promised while it owns the file.
6. Import edited text.
7. Restore terminal mode even on editor failure.
8. Mark dirty and save once.

## 15. Attachments

Composer:

- Open a path-entry overlay; no file browser in v1.
- Expand `~` in Post rather than through a shell.
- Validate that the file exists, is a regular readable file, and has an acceptable size.
- Display filename and human-readable size.
- Allow removal before send.
- Handle duplicate filenames deterministically.

Reader:

- List filename, MIME type when known, and size.
- Save to an explicitly entered or configured downloads path.
- Avoid silent overwrite; prompt or generate a collision-safe filename.
- Optionally open using `open` on macOS or `xdg-open` on Linux.
- Launch opener commands directly, never through a shell.

## 16. Search and pagination

- `/` focuses search.
- `Enter` sends the string unchanged to the backend search operation.
- Do not implement a Gmail search parser.
- Search the current mailbox for v1 unless the verified Himalaya shared API offers a reliable account-wide option.
- Search results reuse the mailbox list component, page state, actions, and reader route.
- Keep explicit pagination with a default page size of 20.
- Display `start–end of total` when total is known; degrade to page number/next availability otherwise.
- Up/Down selects within a page; Left/Right changes page.
- Empty results are valid and must not panic.

## 17. Configuration

Expose one user-facing TOML file. Reuse Himalaya account/credential configuration rather than inventing another account system. Add Post-owned sections if Himalaya safely ignores them; otherwise filter/proxy configuration internally while keeping one canonical user file.

Target shape, subject to the backend spike:

```toml
# Himalaya account/backend configuration remains here.

[post]
account = "personal"
mouse = true

[post.mail]
page_size = 20
refresh_interval_seconds = 60

[post.composer]
editor = "builtin"           # or "$EDITOR" / explicit command
autosave_delay_ms = 2000

[post.attachments]
downloads_dir = "~/Downloads"

[post.theme]
name = "default"
```

Startup validation must report all detected issues together where practical:

- Config file parse failure
- Missing selected account
- Missing Himalaya executable
- Unsupported/incompatible Himalaya version
- Invalid refresh/page/autosave values
- Invalid editor command
- Invalid downloads path

Never print secrets in validation errors.

## 18. Responsive UI and theme

Use semantic theme tokens, not scattered literal colors:

```rust
pub struct Theme {
    pub background: Color,
    pub surface: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub warning: Color,
    pub error: Color,
    pub selection: Color,
    pub unread: Modifier,
}
```

Reference layout is approximately 152×40. Define and snapshot three modes:

- **Full** (roughly 120+ columns): sidebar, primary and secondary metadata.
- **Compact** (roughly 90–119): hide sidebar or secondary columns/snippets as needed.
- **Too small**: render a clear minimum-size message rather than overlapping widgets.

No Nerd Font dependency. Use ordinary Unicode with ASCII-safe fallbacks where useful. Support no-color behavior if Crossterm reports limited capabilities.

## 19. Implementation phases

Each phase must end with formatting, linting, tests, and a runnable/demoable state. Use `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features` unless the repository defines equivalent commands.

### Phase 0 — Repository and backend discovery

**Goal:** eliminate Himalaya/config/draft uncertainty before dependent work.

Tasks:

- Inspect the existing repository; initialize one binary crate only if absent.
- Record supported Rust and Himalaya versions.
- Add a minimal CLI probe or integration harness.
- Discover the exact structured commands/JSON for mailboxes, envelopes, message retrieval, flags, search, attachments, draft storage, send, reply, and forward.
- Generate JSON Schemas when Himalaya supports it; save representative output as fixtures.
- Verify whether `[post]` keys are tolerated in the same TOML file.
- Prove child-process cancellation.
- Prove draft create/update/replace/delete behavior.
- Characterize send success versus Sent-copy failure.
- Write short ADRs for the backend boundary and selected draft strategy.

Acceptance:

- [ ] A test/probe invokes Himalaya without shell interpolation.
- [ ] Representative JSON is captured and secrets are removed.
- [ ] Draft replacement has a documented safe algorithm.
- [ ] Config coexistence has a documented solution.
- [ ] Ambiguous send outcomes can be identified or conservatively classified as unknown.

### Phase 1 — Application shell and deterministic state core

**Goal:** a safe Ratatui program with a testable reducer and fake data.

Files/responsibilities:

- `runtime/terminal.rs`: enter/restore terminal and panic safety
- `runtime/events.rs`: Crossterm event stream and ticks
- `app/action.rs`, `state.rs`, `reducer.rs`, `route.rs`, `focus.rs`
- `ui/layout.rs`, `theme.rs`, topbar/sidebar/statusbar
- logging setup that never writes into the TUI

Acceptance:

- [ ] App opens and exits cleanly with `Esc`/quit action.
- [ ] Terminal restores after Ctrl+C and induced panic test.
- [ ] Reducer is I/O-free and unit tested.
- [ ] Mock mailbox screen renders at full, compact, and too-small sizes.
- [ ] No `j`/`k` or help shortcut is shown.

### Phase 2 — Himalaya adapter and first Inbox vertical slice

**Goal:** browse real mailboxes and page 1 of real messages.

Tasks:

- Implement domain models and Himalaya-private DTOs/mappers.
- Implement `MailBackend` for mailbox and message-list operations.
- Load mailboxes on startup and select Inbox/first usable mailbox.
- Render message sender, subject, snippet if available, date, read/star/attachment states.
- Implement explicit previous/next pagination.
- Add a fake Himalaya executable for contract tests.

Acceptance:

- [ ] Himalaya DTOs are not referenced by app/UI modules.
- [ ] Empty, partial, malformed, and non-UTF-8-ish output paths fail safely.
- [ ] Selection remains visible and valid across movement and resize.
- [ ] First/last page boundaries do not issue invalid requests.
- [ ] Fake backend tests assert exact argv/stdin handling.

### Phase 3 — Operation manager, cancellation, and error modal

**Goal:** all backend work is responsive and recoverable.

Tasks:

- Add typed effects, operation registry, `OperationId`, and task result channel.
- Add subprocess kill-on-cancel behavior.
- Reject stale/superseded results.
- Build spinner and scrollable Retry/Dismiss modal.
- Add sanitization and typed `RetrySpec`.
- Model ambiguous send result even before composer exists.

Acceptance:

- [ ] Slow fake operation does not block rendering/input.
- [ ] `Esc` kills foreground work and returns to the prior stable state.
- [ ] A slower old mailbox request cannot replace a newer mailbox result.
- [ ] Retry replays equivalent typed intent with a new operation ID.
- [ ] Modal detail scrolls and contains no fixture secrets.

### Phase 4 — Single-message reader and core actions

**Goal:** open, render, navigate back, and mutate one message.

Tasks:

- Fetch and parse one message.
- Build reader header/body/attachment/action layout from `viewer.html`.
- Remove thread count, collapsed messages, and thread navigation.
- Restore exact prior list/search state on `Esc`.
- Mark unread mail read after successful load.
- Implement archive, trash, star/unstar, and mark read/unread.

Acceptance:

- [ ] Reader shows exactly one message.
- [ ] Reader handles missing subject/from/body.
- [ ] Back restores page, selection, focus, and scroll.
- [ ] Actions update UI only after confirmation initially.
- [ ] Failure leaves a coherent state and opens Retry/Dismiss.

### Phase 5 — MIME and HTML rendering subsystem

**Goal:** trustworthy, width-aware terminal rendering for real email.

Tasks:

- Parse raw MIME with a maintained library.
- Implement body selection and HTML fallback.
- Convert renderer output to app-owned `RichText`.
- Style headings, lists, links, quotes, pre/code, tables, rules, and image alt text.
- Disable remote fetches.
- Reflow on width change.
- Build the fixture corpus and snapshots.

Acceptance:

- [ ] Plain, HTML, and multipart messages render deterministically.
- [ ] Malformed HTML falls back without crashing.
- [ ] Remote images/resources cause no network requests.
- [ ] Long URLs/Unicode/RTL never panic or corrupt terminal layout.
- [ ] Snapshot corpus covers every case in section 13.

### Phase 6 — Built-in composer and safe draft autosave

**Goal:** compose/edit/leave/reopen a continuously saved draft.

Tasks:

- Implement field focus and `ratatui-textarea` body editing.
- Add address parsing/validation and Cc/Bcc toggles.
- Implement local draft model, revision tracking, and two-second debounce.
- Implement the safe remote strategy chosen in Phase 0.
- Force save when leaving.
- Implement explicit, confirmed discard.
- Show `Unsaved changes`, `Saving…`, `Draft saved · HH:MM`, and `Save failed`.

Acceptance:

- [ ] Enter inserts newline; Ctrl+Enter is reserved for send.
- [ ] Esc leaves without discarding.
- [ ] Saving revision N cannot mark later revision N+1 clean.
- [ ] Edits during a save trigger another save.
- [ ] Crash/restart preserves the last safe draft under the selected strategy.
- [ ] Autosave failure opens Retry/Dismiss and retains content.

### Phase 7 — MIME construction, send, reply, reply-all, forward

**Goal:** send new mail and correctly related responses.

Tasks:

- Serialize RFC 5322/MIME through a library.
- Send serialized bytes through Himalaya stdin/API contract.
- Seed reply/forward drafts from verified Himalaya behavior or Post logic.
- Preserve reply headers.
- Deduplicate reply-all recipients and exclude self.
- On confirmed send, leave composer and remove/resolve its draft.
- Surface ambiguous outcomes with duplicate-send warning.

Acceptance:

- [ ] Integration fixtures parse sent output back into expected headers/body.
- [ ] Reply and reply-all recipient tests cover duplicates and self-addresses.
- [ ] Forward representation is fixture-tested.
- [ ] Ctrl+Enter sends only from composer.
- [ ] Failed send keeps the draft intact.
- [ ] Ambiguous send never claims definite failure or success.

### Phase 8 — Attachments

**Goal:** attach outgoing files and save/open incoming files safely.

Tasks:

- Add path-entry overlay, validation, attachment chips, and removal.
- Include outgoing attachments in MIME.
- List incoming attachment metadata.
- Implement safe save path and collision handling.
- Implement macOS `open` and Linux `xdg-open` adapters.

Acceptance:

- [ ] Spaces/special characters in paths never invoke a shell.
- [ ] Missing/unreadable files produce retryable detailed errors.
- [ ] Existing downloads are not silently overwritten.
- [ ] MIME round-trip preserves filename, media type, bytes, and size.

### Phase 9 — Search and refresh

**Goal:** backend search plus stable automatic/manual refresh.

Tasks:

- Reuse message-list component/state for search results.
- Pass the query unchanged to Himalaya.
- Add explicit search pagination.
- Implement configurable timer and `Ctrl+R`.
- Preserve selection/page/focus by message ID.
- Suppress repeated identical automatic errors.

Acceptance:

- [ ] Empty and special-character queries follow verified backend behavior.
- [ ] Search results open reader and return to the same results state.
- [ ] New mail appearing above selection does not move the user's logical selection.
- [ ] Timer does not refresh while a conflicting foreground operation is active.
- [ ] Manual refresh remains available after auto-refresh failure.

### Phase 10 — Mouse, responsive polish, and config completion

**Goal:** complete v1 interaction and visual fidelity.

Tasks:

- Record widget rectangles during render and translate mouse events into actions.
- Add click/wheel behavior from section 10.
- Complete semantic theming and full/compact/too-small layouts.
- Finalize one-file configuration and validation.
- Match mockup density and hierarchy without reintroducing out-of-scope elements.

Acceptance:

- [ ] Every mouse action has a keyboard equivalent.
- [ ] Resize never panics or leaves an invalid selection/cursor.
- [ ] Snapshot tests cover at least 152×40, 120×30, 90×25, and too-small.
- [ ] App works without a Nerd Font.
- [ ] Invalid config reports actionable sanitized details.

### Phase 11 — External editor

**Goal:** optional `$EDITOR` flow without terminal corruption or data loss.

Tasks and acceptance follow section 14 exactly, including secure temporary files, terminal restoration, import, and a single save after editor exit.

### Phase 12 — Integration hardening and release gate

**Goal:** prove v1 on macOS and Linux.

Tasks:

- Add temporary Maildir/Himalaya integration suite where supported.
- Run backend contract tests with fake Himalaya.
- Run reducer, MIME/HTML, snapshot, and cancellation tests.
- Test missing executable, invalid config, malformed JSON, timeouts, killed child, network failure, invalid attachment, corrupt MIME, send ambiguity, and panic restoration.
- Add README installation, configuration, shortcuts, known limitations, and troubleshooting.
- Run CI on macOS and Linux.
- Measure normal CLI latency before considering Pimalaya or session reuse.

Acceptance:

- [ ] All v1 definition-of-done items pass.
- [ ] CI passes on macOS and Linux.
- [ ] No secrets appear in logs, snapshots, fixtures, or error dialogs.
- [ ] Real-terminal smoke test confirms restoration and no stdout corruption.
- [ ] Performance is measured; architecture is not rewritten without evidence.

## 20. Test strategy

| Layer | Technique | Required coverage |
| --- | --- | --- |
| Domain/reducer | Unit tests | navigation, routes, focus, selection, stale results, overlays, draft revisions |
| Backend adapter | Fake `himalaya` executable | exact argv, stdin, JSON mapping, stderr, exit codes, cancellation |
| MIME/rendering | `.eml` fixtures + snapshots | HTML/plain selection, Unicode, malformed input, attachments, replies |
| UI | Ratatui `TestBackend` snapshots | mailbox, reader, composer, search, modal, spinner, responsive layouts |
| Integration | Temporary Himalaya Maildir config | listing, reading, flags, drafts, search where supported |
| Platform | macOS/Linux CI + smoke tests | terminal lifecycle, opener/editor command selection |

Avoid brittle snapshots of clocks/spinners by injecting time and animation state. Normalize paths and dates in fixtures.

## 21. Security and privacy checklist

- [ ] No shell invocation for Himalaya, editor, opener, or attachment handling.
- [ ] Secrets are redacted before logging and before error-modal display.
- [ ] Remote HTML resources are never fetched.
- [ ] Attachment saves prevent path traversal and silent overwrite.
- [ ] Temporary editor files use restrictive permissions and are cleaned up.
- [ ] Raw emails are not logged by default.
- [ ] Panics restore terminal state.
- [ ] Commands and config errors do not echo credential values.
- [ ] Cancellation terminates owned children without broad process killing.

## 22. Known risks and decision gates

| Risk | Required response |
| --- | --- |
| Himalaya draft update semantics create duplicates | Resolve in Phase 0; use safe local journal/add-before-delete fallback |
| CLI invocation opens a new connection and feels slow | Measure after vertical slices; consider supported session reuse/local store before Pimalaya |
| Send result is ambiguous | Preserve draft, classify outcome, warn retry may duplicate |
| Himalaya JSON changes | Private DTOs, schema/fixture contract tests, startup version check |
| HTML email is malformed or too wide | Robust parser, safe fallback, width-aware rich model, fixture corpus |
| One TOML file rejects `[post]` sections | Keep one canonical file and provide Himalaya a filtered temporary/config view |
| Archive/trash semantics vary by provider | Resolve aliases/capabilities in backend; never hard-code in UI |
| Auto-refresh races with navigation | Operation IDs, context keys, stale-result rejection, selection by ID |

Do not start a Pimalaya backend in v1 unless measured Himalaya CLI behavior makes the product unusable and the user explicitly approves the scope change.

## 23. v1 definition of done

Post v1 is complete when a user with one valid Himalaya account can:

- [ ] Start the app on macOS or Linux and browse backend-provided mailboxes.
- [ ] Navigate entirely with arrows, Enter, Esc, Tab, and documented shortcuts.
- [ ] Page through, refresh, and search message lists without losing selection.
- [ ] Open one message and read plain-text or rendered HTML content.
- [ ] Archive, trash, mark read/unread, and star/unstar.
- [ ] Compose, autosave, leave, reopen, explicitly discard, and send a draft.
- [ ] Reply, reply-all, and forward with correct addressing/thread headers.
- [ ] Add outgoing attachments and save/open incoming attachments safely.
- [ ] Cancel foreground backend work with Esc.
- [ ] See detailed sanitized Retry/Dismiss errors for every failed operation.
- [ ] Use basic mouse interaction if enabled.
- [ ] Optionally edit the message body using the configured external editor.
- [ ] Recover terminal state after quit, Ctrl+C, failure, or panic.

The release must not contain threads, multiple-account switching, label management, settings/help UI, Vim keys, offline mail sync, AI rendering, or provider-specific mail code.

## 24. Instructions for the implementation agent

1. Read this entire plan and inspect the repository plus all three mockups before editing.
2. Start at Phase 0. Do not begin with a large UI rewrite.
3. Maintain a short checklist in the repository or task tracker and mark acceptance items only after verification.
4. Preserve existing user changes and repository conventions.
5. Prefer the smallest vertical slice that ends in a runnable state.
6. Add tests with each behavior, not as a final cleanup pass.
7. Run formatting, linting, and relevant tests after every phase.
8. If a plan assumption conflicts with verified Himalaya behavior, document the evidence, choose the least expansive compatible design, and surface the decision instead of hiding it.
9. Do not add out-of-scope features opportunistically.
10. At each handoff report: files changed, behavior completed, commands/tests run, unresolved risks, and the next phase.

### First implementation task

Execute **Phase 0 only**:

- inspect or initialize the crate;
- identify the installed/target Himalaya CLI version;
- build a non-shell subprocess probe;
- capture sanitized structured fixtures/schemas;
- prove cancellation;
- determine safe draft replacement and config coexistence;
- characterize ambiguous send outcomes;
- write the backend and draft ADRs;
- run tests and report findings before starting Phase 1.

