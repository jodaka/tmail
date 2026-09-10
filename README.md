# Tmail

A Gmail-inspired, keyboard-first terminal email client built in Rust with
Ratatui, backed by the [Himalaya CLI](https://pimalaya.org) for all mail
protocols, accounts, and credentials.

## Installation

### Homebrew

Tap and install (the tap formula is synced from `Formula/tmail.rb` on every
release and pulls the prebuilt binaries from the GitHub releases):

```sh
brew tap jodaka/tap https://github.com/jodaka/homebrew-tap
brew install jodaka/tap/tmail   # installs himalaya automatically
```

### From source

1. Install the Himalaya CLI (v2.x, with the backend feature you need):

   ```sh
   brew install himalaya        # macOS (Homebrew)
   cargo install himalaya       # any platform with Rust
   ```

2. Build Tmail from source:

   ```sh
   git clone <this repository>
   cd tmail
   cargo build --release
   ```

   The binary lands at `target/release/tmail`.

3. Run it (`cargo run --release` from the checkout, or copy the binary
   anywhere):

## Requirements

- macOS (required) / Linux (supported); Windows is out of scope
- Stable Rust (2024 edition)
- `himalaya` CLI v2.x installed and on `PATH` (Tmail checks at startup and
  refuses to start with an actionable error if it is missing)

## Running

```sh
cargo run --release                          # himalaya's default config
cargo run --release -- path/to/config.toml   # explicit config file
TMAIL_CONFIG=path/to/config.toml cargo run --release
tmail --configure                            # account setup wizard
```

## Configuration

Tmail and Himalaya share **one** TOML file. Himalaya's `[accounts.*]`
blocks (servers, credentials) keep their own format and are never touched
by Tmail; Tmail reads its own `[tmail…]` tables from the same file (himalaya
2.1.0 tolerates the unknown root tables).

### Which file is loaded

Exactly one of, in priority order:

1. The path given as the first CLI argument (`tmail path/to/config.toml`)
2. The `TMAIL_CONFIG` environment variable
3. `~/.config/himalaya/config.toml` (if it exists)
4. `~/Library/Application Support/himalaya/config.toml` (if it exists)

If none exist, Tmail runs with defaults and lets himalaya pick its own
default config. **Only the file Tmail actually loads is used — settings in
any other file are ignored.**

### All `[tmail]` options

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `[tmail].account` | string | none | Name of the `[accounts.<name>]` table Tmail drives (forwarded to himalaya as `-a`). When absent, Tmail uses the account himalaya itself would pick: the one with `default = true`, else the sole account. **If set, the name must match an existing `[accounts.<name>]` table or startup fails.** |
| `[tmail].mouse` | bool | `false` | **Mouse support (off by default).** When `true`, Tmail enables terminal mouse capture and you can click mailboxes, message rows, the search field, the Compose button, links and attachment chips in the reader, composer controls, and modal buttons, and scroll with the wheel. See [Mouse](#mouse) for exact behavior. Capture changes what terminal text selection does, so it is opt-in. |
| `[tmail].view_mode` | string | `"compact"` | Message-list density. `"compact"` (default) draws one line per message; `"comfortable"` splits consecutive messages with a faint horizontal separator, so each message takes two lines — fewer messages fit on screen, with more negative space between rows. |
| `[tmail].status_timeout` | integer | `0` | Seconds a status message stays up in the bottom-right corner before it fades into the background (over the last 0.3 s) and clears. `0` keeps a message until the next one replaces it. |
| `[tmail.mail].page_size_auto` | bool | `true` | Size each page to the number of message rows the terminal can show (ticket kjfq): the whole page fits the list without scrolling, and resizing re-loads the page. When `true`, `page_size` is ignored. |
| `[tmail.mail].page_size` | integer | `50` | Rows per page of the message list with `page_size_auto = false` (explicit pagination with ←/→). Must be positive. A page longer than the list shows a vertical scrollbar. |
| `[tmail.mail].refresh_interval_seconds` | integer | `60` | Periodic background refresh; `0` disables the timer. Never preempts foreground work or the composer. |
| `[tmail.composer].editor` | string | `"builtin"` | `"builtin"`, `"$EDITOR"` (resolved from the environment), or a plain command like `nvim` (program + arguments, **no shell metacharacters** — Tmail never spawns a shell). The external-editor flow itself ships in Phase 11; the value is validated at startup either way. |
| `[tmail.composer].autosave_delay_ms` | integer | `2000` | Draft autosave debounce for the builtin editor. Accepted range: 100–600000. |
| `[tmail.attachments].downloads_dir` | string | `$HOME/Downloads` | Directory used by *save attachment*. Must be absolute or start with `~/` (Tmail expands `~` itself, never via a shell). May not exist yet; must not be an existing file. |
| `[tmail.theme].name` | string | `"default"` | Theme name: `"default"` (dark) or `"light"` (ticket wrs7). Set the `NO_COLOR` environment variable (non-empty) to render without any colors at all — it also ignores theme overrides. |
| `[tmail.themes.<name>]` | table | none | Extra named themes for runtime switching (ticket z0s4): same color tokens as `[tmail.theme]` (no `name` key — the table's name is the theme's name), values are hex colors applied over the dark reference palette. Press `t` in Tmail to pick one from the theme dialog: built-ins first, then these alphabetically by name. A theme named `default` or `light` replaces that built-in. Switching is session-only — the config file is never rewritten. |
| `[tmail.keybindings.<context>]` | table of key lists | built-in defaults | Reassign, extend, or unbind keyboard shortcuts per context — `global`, `list`, `reader` (the context column of the [shortcut docs](docs/shortcusts.md)). An action's list **replaces** its default keys; an empty list unbinds the action. Key syntax: `+`-joined `ctrl`/`alt` modifiers then a named key (`esc`, `enter`, `tab`, `backtab`, `backspace`, `delete`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`, `right`, `space`, `f1`–`f12`), a glyph (`↑ ↓ ← → ⌫ ↵`), or any single character (`j`, `?`, `]`; `"S"` is the shifted character). A key already bound to another action in the same context is refused with a startup warning; the escape hatches (`cancel`, `activate`, `focus_next`, `focus_previous`, `quit`) always keep at least one binding. The full annotated default list ships in `config.example.toml`. |
| `[tmail.ui].clock` | bool | `false` | Show the date/time clock in the top-right corner (ticket w7f5). Off by default. |
| `[tmail.cache].max_messages` | integer | `50` | How many viewed messages to keep in Tmail's on-disk cache (ticket haeb) so previously opened mail renders instantly. Least-recently-used entries are evicted first; `0` disables message caching. |
| `[tmail.cache].max_bytes` | integer | `10485760` | Total size cap in bytes for the viewed-message cache (default 10 MiB, ticket haeb). Messages larger than the whole budget are never cached. |

Config example is available in `config.example.toml`.

### Theming

Pick a built-in theme with `[tmail.theme].name` (`"default"` for the dark
reference look, `"light"` for a paper variant), and fine-tune any of the
semantic color tokens right in the same file with hex colors:

```toml
[tmail.theme]
name = "default"
background = "#0a101e"   # #rrggbb or the short #rgb form
accent = "#8ab4f8"
error = "#ff6b5e"
```

Overridable tokens: `background`, `surface`, `border`, `text`,
`text_soft`, `muted`, `dim`, `snippet` (the faded message preview in the
list), `accent`, `accent_bg`, `bulk_selected_bg`
(the Space-marked row highlight), `warning`, `error`, `selection`. Unknown
tokens or malformed colors fail startup validation like any other config
problem; `NO_COLOR` overrides everything and renders with terminal
defaults only.

The full default palette is written out with per-token usage notes in
[`docs/default-theme.toml`](docs/default-theme.toml) — a ready-to-paste
starting point for your own tuning.

#### Multiple themes and the theme picker

Define any number of extra themes as `[tmail.themes.<name>]` tables and
press `t` in Tmail to open the theme picker at runtime (built-ins first,
then yours alphabetically by name):

```toml
[tmail.themes.nord]
background = "#2e3440"
accent = "#88c0d0"
text = "#eceff4"

[tmail.themes.warm]
background = "#262220"
accent = "#e0916c"
```

The picker is a small dialog listing every theme: `↑`/`↓` move the
highlight, and the highlighted theme **previews at once** — the whole
screen recolors while you navigate. `Enter` keeps the previewed palette
(the status line names it); `Esc` closes the dialog and restores the
palette you started with.

Unspecified tokens keep the dark reference values; a theme named
`default` or `light` replaces that built-in in the dialog. Switching is
session-only — the shared config file is never rewritten.

### Startup validation

Before the UI starts, Tmail validates the whole file and reports **every**
detected problem together (never just the first), then refuses to start:

- file exists but is not valid TOML (parse errors are sanitized)
- `[tmail].account` names a missing `[accounts.*]` table
- invalid `page_size`, `refresh_interval_seconds`, or `autosave_delay_ms`
- invalid `editor` (empty, unknown program, shell metacharacters, `$EDITOR` unset)
- invalid `downloads_dir` (relative path, or an existing file)
- unknown `[tmail.theme].name`
- `himalaya` executable not found on `PATH`

Validation errors never echo file contents or secrets.

### Himalaya account tables (reference)

Tmail reads, from himalaya's own blocks:

- `[accounts.<name>].email` — used as the `From` identity of drafts and to
  exclude yourself from reply-all
- `[accounts.<name>].display-name` — display identity
- `[accounts.<name>].default` — account selection when `[tmail].account` is unset
- `[accounts.<name>.mailbox.alias]` — maps semantic roles (`inbox`, `sent`,
  `drafts`, `trash`, `archive`) to the account's real folder names; without
  an alias, archive/trash resolve from what the account actually exposes

Keyboard shortcuts described in [docs/shortcusts.md](docs/shortcusts.md)

## Account configuration wizard

Tmail needs a working himalaya account to be useful. Instead of
hand-editing the config file, the built-in wizard sets one up in the app:

```sh
tmail --configure            # open the wizard at any time (optionally: --configure path/to/config.toml)
```

It also starts automatically on first run, when no usable `[accounts]`
entry is found. The flow:

1. **Email address** — the address you are configuring.
2. **Server settings** — Tmail discovers IMAP/SMTP endpoints for the
   domain in-process (Mozilla Thunderbird autoconfig, PACC, DNS SRV
   RFC 6186, fixed rules for Gmail/Outlook) and shows a ranked list. POP
   results are discarded (himalaya has no POP3 backend). If nothing is
   found — or the suggestion is wrong — press `e` and enter the servers
   manually (`imaps://host:993` style).
3. **Identity** — an optional display name.
4. **Sign-in** — a username plus your choice of password storage: either
   store the password in the config file (`password.raw`, file mode
   `0600`) or store a command that prints it (`password.cmd`, e.g.
   `pass show mail/gmail`). Tmail never executes that command — himalaya
   does, at connection time.
5. **Test** — the credentials are verified with a real `himalaya mailbox
   list` against a temporary owner-only config file. Nothing containing
   the credential is written to the real config until the test passes.
6. **Aliases** — the special-folder roles (`inbox`, `sent`, `drafts`,
   `trash`, `archive`) are derived from the tested mailbox listing (a
   Gmail preset plus generic name heuristics), shown for confirmation,
   and saved.

The account is merged into the shared config file with
format-preserving edits: existing accounts, `[tmail]` tables, comments
and ordering survive. A freshly created config file gets mode `0600`.
`Esc` steps back at every point and `Ctrl+C` quits; in `--configure`
mode the saved path is printed on success (exit 0) and
`tmail: configuration not changed` on cancel (exit 1).

**Gmail note:** IMAP with a normal Google password requires an **app
password** (a Google account with 2FA). The wizard shows this hint on
the sign-in screen.

**Privacy note:** discovery queries public infrastructure — the
Thunderbird ISPDB, `autoconfig` well-known URLs, DNS resolvers — and
therefore reveals the *domain* (and in some autoconfig query strings the
full address) to those services. Discovery only runs when you submit the
email screen; tests and smoke runs never touch the network
(`TMAIL_FAKE_DISCOVERY=1` selects a canned fake discoverer).

## External editor

With `[tmail.composer].editor` set to `"$EDITOR"` (or an explicit command
like `nvim`), press `Ctrl+E` inside the composer to edit the draft body in
your own editor:

1. The draft is saved first (journal + remote), so nothing can be lost.
2. Tmail suspends its UI: raw mode and the alternate screen are left, and
   your editor takes over the full terminal.
3. The body travels through a secure temporary file (owner-only
   permissions, removed afterwards). The editor gets the file path as its
   last argument.
4. Tmail waits for the editor to exit — no background autosave runs while
   it owns the file — then imports the text and saves once.
5. The terminal is restored even if the editor fails; a failed run
   imports nothing and the draft stays intact.

With the default `editor = "builtin"`, `Ctrl+E` does nothing.

## Cache

Tmail keeps a small on-disk cache (in its data directory, scoped per
account) so warm starts and mailbox switches render instantly and refresh
in the background:

- **Mailbox listing** — the sidebar renders from the last known listing;
- **Message-list pages** — the last loaded page per mailbox (and per
  search query);
- **Viewed messages** — full messages you have opened render instantly on
  re-open.

Cache reads are conservative: only exact-identity hits are used, unparsable
entries are ignored, and every successful backend load overwrites the
cached data — the fresh value always wins. The viewed-message cache is
limited by `[tmail.cache].max_messages` and `[tmail.cache].max_bytes`
(least-recently-used eviction; see [Configuration](#configuration)). For
slow IMAP hosts, [sirup](https://github.com/pimalaya/sirup) can additionally
amortize the per-invocation connection cost; see `cache.md` for details.

## Bulk selection

Select messages with `Space` (or by clicking the `[ ] Inbox` label in the
list header, or `Ctrl+A` for every visible message). While any message is
marked, the status bar shows how many are selected and the bulk operations:

- `[delete]` — trash every selected message (`Delete`)
- `[archive]` — archive every selected message (`e`)
- `[read]` — mark every selected message read (`i`)
- `[unread]` — mark every selected message unread (`u`)

The buttons are clickable; the bracketed keys do the same from the list.
Marks ride the message id, so they survive paging and refreshes; switching
mailboxes, leaving search, or pressing `Esc` (with nothing to cancel or
close) clears the selection. Marked rows carry a distinct highlight fill.
`Enter` still opens the focused message; the only place it touches the
selection is on the focused `[ ] Inbox` toggle itself.

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
- There is no all-fields `text` filter; plain words are Tmail's shorthand
  for `(from "…") or (subject "…") or (body "…")`.
- See [Known limitations](#known-limitations) for backend-specific search
  caveats (non-ASCII text on IMAP, text search on Maildir).

## Mouse

Mouse support is **off by default** — enable it with `[tmail] mouse = true`
(see [Configuration](#configuration)), or press `m` inside Tmail to toggle
capture at any time. With it enabled:

- **Message row** — first click selects the row; clicking the already
  selected row opens it (the same select → Enter rhythm as the keyboard).
- **Folder (sidebar)** — first click selects; clicking the selected folder
  switches to it.
- **Compose button** — opens the composer (same as `c`).
- **Search field** — focuses it (same as `/`).
- **Links (reader)** — first click focuses the link; clicking the focused
  link opens it in the system browser (same as Tab + `Enter`).
- **Attachment chips (reader)** — first click focuses the chip; clicking
  the focused chip opens it (same as `o`).
- **Composer** — clicking a field focuses it; clicking Cc/Bcc toggles,
  `+ attach`, a chip, Send, or Discard activates that control.
- **Modal buttons** — Retry/Dismiss and Discard/Keep press like
  Tab+Enter.
- **Wheel** — scrolls the focused area (list, sidebar, reader, or modal).

Every mouse action has a keyboard equivalent; nothing is mouse-only.
Middle/right click and dragging do nothing.

### Text selection while the mouse is enabled

Enabling mouse capture tells the terminal to send clicks to Tmail instead
of using them for selection — that is how click support works in every
TUI. You keep two ways to select text:

1. **Hold `Shift` while clicking or dragging.** Standard convention
   (iTerm2, Terminal.app, Alacritty, kitty, WezTerm, foot, GNOME Terminal,
   Windows Terminal, …): the terminal handles the selection itself and
   Tmail never sees those events.
2. **Press `m`.** This turns mouse capture off entirely — the terminal
   behaves exactly as if Tmail had no mouse support, so plain click-drag
   selects text. Press `m` again to re-enable clicks. The status bar
   always shows which mode you are in.

## Known limitations

**v1 scope** (by design, per `TMAIL_IMPLEMENTATION_PLAN.md` §23): no
conversation threads, no multiple-account switching, no label management,
no settings/help UI, and no offline sync.

**Search:**

- Non-ASCII search text (e.g. Cyrillic) currently fails with
  `IMAP SEARCH failed: BAD` — the IMAP server rejects it via himalaya
  2.1.0. Folder names and message content render fine; only searching
  *in* non-ASCII text is affected.
- On local Maildir accounts (himalaya `maildir` backend), text search
  matches nothing: himalaya delegates text filters to the server, and a
  Maildir has none. Flag filters do work locally (`flag flagged`,
  `not flag seen`). Listing, reading, flags, and drafts all work on
  Maildir (covered by `tests/maildir_integration.rs`).

**Mouse:** with capture enabled, plain click-drag belongs to Tmail; hold
`Shift` or press `m` to select text ([details](#mouse)).

## Development

The module map — one page on the layers and data flow (reducer loop,
backend trait, UI/input/config slices) — is [docs/architecture.md](docs/architecture.md).

```sh
cargo build
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
python3 fixtures/smoke/ci_smoke.py --bin target/debug/tmail   # pty smoke (CI runs it too)
```

## Repository layout

- `docs/adr/` — architecture decision records
- `docs/phase-*.md` — per-phase delivery checklists
- `.github/workflows/ci.yml` — macOS + Linux CI (fmt, clippy, tests, pty smoke)
- `fixtures/himalaya/` — sanitized probe fixtures, schemas, seed/sink helpers
- `fixtures/smoke/` — committed pty smoke: fake himalaya + CI driver
- `src/backend/` — `MailBackend` trait + Himalaya CLI adapter (DTOs private)
- `src/config/` — shared one-file configuration (`[tmail]` + aliases)
- `src/input/` — keyboard and mouse → action translation
- `tests/fake_himalaya.rs` — fake `himalaya` executable for contract tests
- `tests/backend_contract.rs` — backend contract test suite
- `TMAIL_IMPLEMENTATION_PLAN.md` — the product/engineering specification

## Additional documentation
`./docs/builtin-be.md` — brief ideas about bundling Himalaya with app
`./docs/shortcuts.md` — keyboard shortcuts list
`./docs/summary.md` — implementation history summary
