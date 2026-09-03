# Post

A Gmail-inspired, keyboard-first terminal email client built in Rust with
Ratatui, backed by the [Himalaya CLI](https://pimalaya.org) for all mail
protocols, accounts, and credentials.

**Status: Phases 0–10 complete** (shell, Himalaya adapter, operation
manager/cancellation, reader, MIME/HTML rendering, composer + draft
autosave, send/reply/forward, attachments, search/refresh, mouse +
responsive polish + config validation). Implementation proceeds phase by
phase per `POST_IMPLEMENTATION_PLAN.md`.

## Requirements

- macOS (required) / Linux (supported); Windows is out of scope
- Stable Rust (2024 edition)
- `himalaya` CLI v2.x installed and on `PATH` (Post checks at startup and
  refuses to start with an actionable error if it is missing)

## Running

```sh
cargo run --release                          # himalaya's default config
cargo run --release -- path/to/config.toml   # explicit config file
POST_CONFIG=path/to/config.toml cargo run --release
```

## Configuration

Post and Himalaya share **one** TOML file. Himalaya's `[accounts.*]`
blocks (servers, credentials) keep their own format and are never touched
by Post; Post reads its own `[post…]` tables from the same file (himalaya
2.1.0 tolerates the unknown root tables).

### Which file is loaded

Exactly one of, in priority order:

1. The path given as the first CLI argument (`post path/to/config.toml`)
2. The `POST_CONFIG` environment variable
3. `~/.config/himalaya/config.toml` (if it exists)
4. `~/Library/Application Support/himalaya/config.toml` (if it exists)

If none exist, Post runs with defaults and lets himalaya pick its own
default config. **Only the file Post actually loads is used — settings in
any other file are ignored.**

### All `[post]` options

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `[post].account` | string | none | Name of the `[accounts.<name>]` table Post drives (forwarded to himalaya as `-a`). When absent, Post uses the account himalaya itself would pick: the one with `default = true`, else the sole account. **If set, the name must match an existing `[accounts.<name>]` table or startup fails.** |
| `[post].mouse` | bool | `false` | **Mouse support (off by default).** When `true`, Post enables terminal mouse capture and you can click mailboxes, message rows, the search field, the Compose button, attachment chips, composer controls, and modal buttons, and scroll with the wheel. See [Mouse](#mouse) for exact behavior. Capture changes what terminal text selection does, so it is opt-in. |
| `[post.mail].page_size` | integer | `20` | Rows per page of the message list (explicit pagination with ←/→). Must be positive. |
| `[post.mail].refresh_interval_seconds` | integer | `60` | Periodic background refresh; `0` disables the timer. Never preempts foreground work or the composer. |
| `[post.composer].editor` | string | `"builtin"` | `"builtin"`, `"$EDITOR"` (resolved from the environment), or a plain command like `nvim` (program + arguments, **no shell metacharacters** — Post never spawns a shell). The external-editor flow itself ships in Phase 11; the value is validated at startup either way. |
| `[post.composer].autosave_delay_ms` | integer | `2000` | Draft autosave debounce for the builtin editor. Accepted range: 100–600000. |
| `[post.attachments].downloads_dir` | string | `$HOME/Downloads` | Directory used by *save attachment*. Must be absolute or start with `~/` (Post expands `~` itself, never via a shell). May not exist yet; must not be an existing file. |
| `[post.theme].name` | string | `"default"` | Theme name. `"default"` is the dark reference theme. Set the `NO_COLOR` environment variable (non-empty) to render without any colors at all. |

### Example

```toml
# ── Himalaya accounts (himalaya owns this format) ──────────────────
[accounts.personal]
email    = "you@example.org"
default  = true
# ... himalaya backend/credential blocks ...

[accounts.personal.mailbox.alias]
inbox   = "INBOX"
sent    = "Sent"
drafts  = "Drafts"
trash   = "[Gmail]/Trash"
archive = "[Gmail]/All Mail"

# ── Post-owned settings ────────────────────────────────────────────
[post]
account = "personal"        # optional; omit to use himalaya's default
mouse   = true              # REQUIRED for mouse support (default: off)

[post.mail]
page_size = 20
refresh_interval_seconds = 60

[post.composer]
editor = "builtin"
autosave_delay_ms = 2000

[post.attachments]
downloads_dir = "~/Downloads"

[post.theme]
name = "default"
```

A runnable copy lives in `config.example.toml`.

### Startup validation

Before the UI starts, Post validates the whole file and reports **every**
detected problem together (never just the first), then refuses to start:

- file exists but is not valid TOML (parse errors are sanitized)
- `[post].account` names a missing `[accounts.*]` table
- invalid `page_size`, `refresh_interval_seconds`, or `autosave_delay_ms`
- invalid `editor` (empty, unknown program, shell metacharacters, `$EDITOR` unset)
- invalid `downloads_dir` (relative path, or an existing file)
- unknown `[post.theme].name`
- `himalaya` executable not found on `PATH`

Validation errors never echo file contents or secrets.

### Himalaya account tables (reference)

Post reads, from himalaya's own blocks:

- `[accounts.<name>].email` — used as the `From` identity of drafts and to
  exclude yourself from reply-all
- `[accounts.<name>].display-name` — display identity
- `[accounts.<name>].default` — account selection when `[post].account` is unset
- `[accounts.<name>.mailbox.alias]` — maps semantic roles (`inbox`, `sent`,
  `drafts`, `trash`, `archive`) to the account's real folder names; without
  an alias, archive/trash resolve from what the account actually exposes

## Keyboard

| Context | Key | Action |
| --- | --- | --- |
| Global | `↑` / `↓` | Move selection / scroll focused area |
| Global | `←` / `→` | Previous / next page (reader: page the viewport) |
| Global | `Enter` | Open / activate focused control |
| Global | `Esc` (or `q`) | Cancel work → close overlay → go back |
| Global | `Tab` / `Shift+Tab` | Next / previous focus |
| Global | `/` | Focus search |
| Global | `c` | Compose |
| Global | `Ctrl+R` | Manual refresh |
| Global | `Ctrl+C` | Quit |
| Global | `m` | Toggle mouse capture on/off (see [Mouse](#mouse)) |
| List/reader | `r` / `a` / `f` | Reply / reply-all / forward |
| List/reader | `e` / `s` / `u` | Archive / star / mark unread |
| List/reader | `Delete` | Trash |
| Reader | `d` / `o`, `Tab` | Save / open attachment, cycle chips |
| Search | printable, `Backspace`, `Enter`, `Esc` | Edit query, submit, leave |
| Composer | `Enter` | Newline in body; activate focused control |
| Composer | `Ctrl+Enter` | Send |
| Composer | `Esc` | Save and leave (never silently discards) |
| Modal | `↑↓` / `Tab` / `Enter` / `Esc` | Scroll, switch button, confirm, dismiss/keep |

## Search

Press `/`, type, press `Enter`. Two query styles:

**Plain words (Gmail-style).** Any text without a filter keyword searches
sender, subject, and body at once:

```text
plati
hello world
```

**Filter syntax (himalaya's search DSL).** Queries that use a filter
keyword pass straight to himalaya, so power filters keep working. Keywords
are lowercase and take a space (not a colon):

```text
from alice@example.org
subject invoices
body "hello world"          # quoted phrases
from bob and subject report
(from bob) or (subject report)
not flag seen
```

Notes and limitations:

- Filter keywords are **lowercase** and space-separated (`from x`, not
  `from:x`) — that is himalaya 2.1.0's grammar.
- There is no all-fields `text` filter; plain words are Post's shorthand
  for `(from "…") or (subject "…") or (body "…")`.
- Non-ASCII search text (e.g. Cyrillic) currently fails with
  `IMAP SEARCH failed: BAD` — the IMAP server rejects it via himalaya
  2.1.0. Folder names and message content render fine; only searching
  *in* non-ASCII text is affected.

## Mouse

Mouse support is **off by default** — enable it with `[post] mouse = true`
(see [Configuration](#configuration)), or press `m` inside Post to toggle
capture at any time. With it enabled:

- **Message row** — first click selects the row; clicking the already
  selected row opens it (the same select → Enter rhythm as the keyboard).
- **Folder (sidebar)** — first click selects; clicking the selected folder
  switches to it.
- **Compose button** — opens the composer (same as `c`).
- **Search field** — focuses it (same as `/`).
- **Reader action row** — Reply / Forward / Archive / Star / Unread /
  Delete / Save / Open are clickable and run exactly what their
  advertised key runs.
- **Attachment chips (reader)** — first click selects the chip; clicking
  the selected chip opens it (same as `o`).
- **Composer** — clicking a field focuses it; clicking Cc/Bcc toggles,
  `+ attach`, a chip, Send, or Discard activates that control.
- **Modal buttons** — Retry/Dismiss and Discard/Keep press like
  Tab+Enter.
- **Wheel** — scrolls the focused area (list, sidebar, reader, or modal).

Every mouse action has a keyboard equivalent; nothing is mouse-only.
Middle/right click and dragging do nothing.

### Text selection while the mouse is enabled

Enabling mouse capture tells the terminal to send clicks to Post instead
of using them for selection — that is how click support works in every
TUI. You keep two ways to select text:

1. **Hold `Shift` while clicking or dragging.** Standard convention
   (iTerm2, Terminal.app, Alacritty, kitty, WezTerm, foot, GNOME Terminal,
   Windows Terminal, …): the terminal handles the selection itself and
   Post never sees those events.
2. **Press `m`.** This turns mouse capture off entirely — the terminal
   behaves exactly as if Post had no mouse support, so plain click-drag
   selects text. Press `m` again to re-enable clicks. The status bar
   always shows which mode you are in.

## Troubleshooting

- **Mouse does nothing** — `[post] mouse = true` is missing from the file
  Post actually loaded (check the [load order](#which-file-is-loaded)), or
  the terminal does not pass mouse events through. You can also just
  press `m` to turn capture on without touching the config.
- **Cannot select text with the mouse** — mouse capture is on; hold
  `Shift` while dragging, or press `m` to release the mouse to the
  terminal ([details](#text-selection-while-the-mouse-is-enabled)).
- **Post refuses to start listing problems** — fix each item it prints in
  the config file; it validates everything up front on purpose.
- **Mailboxes show but archive/trash fail** — add
  `[accounts.<name>.mailbox.alias]` entries matching your provider's
  folder names.
- **Terminal is a mess after a crash** — Post restores the terminal on
  exit, error, and panic; if a hard kill left it broken, run `reset`.

## Development

```sh
cargo build
cargo run --bin probe -- <config.toml>   # Phase 0 backend probe harness
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
```

## Repository layout

- `docs/adr/` — architecture decision records
- `docs/phase-*.md` — per-phase delivery checklists
- `fixtures/himalaya/` — sanitized probe fixtures, schemas, seed/sink helpers
- `src/backend/` — `MailBackend` trait + Himalaya CLI adapter (DTOs private)
- `src/config/` — shared one-file configuration (`[post]` + aliases)
- `src/input/` — keyboard and mouse → action translation
- `src/bin/probe.rs` — subprocess probe (argv-only, stdin, cancellation)
- `tests/fake_himalaya.rs` — fake `himalaya` executable for contract tests
- `tests/backend_contract.rs` — backend contract test suite
- `POST_IMPLEMENTATION_PLAN.md` — the product/engineering specification

## License

TBD
