# Configuration

Tmail and Himalaya share **one** TOML file. Himalaya's `[accounts.*]`
blocks (servers, credentials) keep their own format and are never touched
by Tmail; Tmail reads its own `[tmail…]` tables from the same file (himalaya
2.1.0 tolerates the unknown root tables).

## Which file is loaded

Exactly one of, in priority order:

1. The path given as the first CLI argument (`tmail path/to/config.toml`)
2. The `TMAIL_CONFIG` environment variable
3. `~/.config/himalaya/config.toml` (if it exists)
4. `~/Library/Application Support/himalaya/config.toml` (if it exists)

If none exist, Tmail runs with defaults and lets himalaya pick its own
default config. **Only the file Tmail actually loads is used — settings in
any other file are ignored.**

## All `[tmail]` options

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `[tmail].account` | string | none | Name of the `[accounts.<name>]` table Tmail drives (forwarded to himalaya as `-a`). When absent, Tmail uses the account himalaya itself would pick: the one with `default = true`, else the sole account. **If set, the name must match an existing `[accounts.<name>]` table or startup fails.** |
| `[tmail].mouse` | bool | `false` | **Mouse support (off by default).** When `true`, Tmail enables terminal mouse capture and you can click mailboxes, message rows, the search field, the Compose button, links and attachment chips in the reader, composer controls, and modal buttons, and scroll with the wheel. See [Mouse](features.md#mouse) for exact behavior. Capture changes what terminal text selection does, so it is opt-in. |
| `[tmail].view_mode` | string | `"compact"` | Message-list density. `"compact"` (default) draws one line per message; `"comfortable"` splits consecutive messages with a faint horizontal separator, so each message takes two lines — fewer messages fit on screen, with more negative space between rows. |
| `[tmail].status_timeout` | integer | `0` | Seconds a status message stays up in the bottom-right corner before it fades into the background (over the last 0.3 s) and clears. `0` keeps a message until the next one replaces it. |
| `[tmail].notifications` | string | `"off"` | New-mail notifications for the periodic background refresh (ticket b28p). `"off"` stays silent; `"bell"` rings the terminal bell (`\x07`); `"on"` shows a desktop notification through `notify-rust` (a single message names its sender and subject, several list the count). Only mail that arrived since the previous background update notifies, and only while the terminal window is unfocused — active users are never interrupted. |
| `[tmail.mail].page_size_auto` | bool | `true` | Size each page to the number of message rows the terminal can show (ticket kjfq): the whole page fits the list without scrolling, and resizing re-loads the page. When `true`, `page_size` is ignored. |
| `[tmail.mail].page_size` | integer | `50` | Rows per page of the message list with `page_size_auto = false` (explicit pagination with ←/→). Must be positive. A page longer than the list shows a vertical scrollbar. |
| `[tmail.mail].refresh_interval_seconds` | integer | `60` | Periodic background refresh; `0` disables the timer. Never preempts foreground work or the composer. |
| `[tmail.composer].editor` | string | `"builtin"` | `"builtin"`, `"$EDITOR"` (resolved from the environment), or a plain command like `nvim` (program + arguments, **no shell metacharacters** — Tmail never spawns a shell). The external-editor flow itself ships in Phase 11; the value is validated at startup either way. |
| `[tmail.composer].autosave_delay_ms` | integer | `2000` | Draft autosave debounce for the builtin editor. Accepted range: 100–600000. |
| `[tmail.attachments].downloads_dir` | string | `$HOME/Downloads` | Directory used by *save attachment*. Must be absolute or start with `~/` (Tmail expands `~` itself, never via a shell). May not exist yet; must not be an existing file. |
| `[tmail.theme].name` | string | `"default"` | Theme name: `"default"` (dark) or `"light"` (ticket wrs7). Set the `NO_COLOR` environment variable (non-empty) to render without any colors at all — it also ignores theme overrides. |
| `[tmail.themes.<name>]` | table | none | Extra named themes for runtime switching (ticket z0s4): same color tokens as `[tmail.theme]` (no `name` key — the table's name is the theme's name), values are hex colors applied over the dark reference palette. Press `t` in Tmail to pick one from the theme dialog: built-ins first, then these alphabetically by name. A theme named `default` or `light` replaces that built-in. Switching is session-only — the config file is never rewritten. |
| `[tmail.keybindings.<context>]` | table of key lists | built-in defaults | Reassign, extend, or unbind keyboard shortcuts per context — `global`, `list`, `reader` (the context column of the [shortcut docs](shortcusts.md)). An action's list **replaces** its default keys; an empty list unbinds the action. Key syntax: `+`-joined `ctrl`/`alt` modifiers then a named key (`esc`, `enter`, `tab`, `backtab`, `backspace`, `delete`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`, `right`, `space`, `f1`–`f12`), a glyph (`↑ ↓ ← → ⌫ ↵`), or any single character (`j`, `?`, `]`; `"S"` is the shifted character). A key already bound to another action in the same context is refused with a startup warning; the escape hatches (`cancel`, `activate`, `focus_next`, `focus_previous`, `quit`) always keep at least one binding. The full annotated default list ships in `config.example.toml`. |
| `[tmail.ui].clock` | bool | `false` | Show the date/time clock in the top-right corner (ticket w7f5). Off by default. |
| `[tmail.cache].max_messages` | integer | `50` | How many viewed messages to keep in Tmail's on-disk cache (ticket haeb) so previously opened mail renders instantly. Least-recently-used entries are evicted first; `0` disables message caching. |
| `[tmail.cache].max_bytes` | integer | `10485760` | Total size cap in bytes for the viewed-message cache (default 10 MiB, ticket haeb). Messages larger than the whole budget are never cached. |

Config example is available in `config.example.toml`.

## Theming

Pick a built-in theme with `[tmail.theme].name` (`"default"` for the dark
reference look, `"light"` for a paper variant), and fine-tune any of the
semantic color tokens right in the same file with hex colors:

```toml
[tmail.theme]
name = "default"
background = "#0d1017"   # #rrggbb or the short #rgb form
accent = "#8ab4f8"
error = "#ff6b5e"
```

Overridable tokens: `background`, `surface`, `border`, `text`,
`text_soft`, `muted`, `dim`, `snippet` (the faded message preview in the
list), `accent`, `marker` (the selected/active row fill), `marker_bar` (its
white left edge bar), `accent_bg`, `bulk_selected_bg`
(the Space-marked row highlight), `warning`, `error`, `selection`. Unknown
tokens or malformed colors fail startup validation like any other config
problem; `NO_COLOR` overrides everything and renders with terminal
defaults only.

The full default palette is written out with per-token usage notes in
[`default-theme.toml`](default-theme.toml) — a ready-to-paste
starting point for your own tuning.

### Multiple themes and the theme picker

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

## Startup validation

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

## Himalaya account tables (reference)

Tmail reads, from himalaya's own blocks:

- `[accounts.<name>].email` — used as the `From` identity of drafts and to
  exclude yourself from reply-all
- `[accounts.<name>].display-name` — display identity
- `[accounts.<name>].default` — account selection when `[tmail].account` is unset
- `[accounts.<name>.mailbox.alias]` — maps semantic roles (`inbox`, `sent`,
  `drafts`, `trash`, `archive`) to the account's real folder names; without
  an alias, archive/trash resolve from what the account actually exposes

Keyboard shortcuts described in [shortcusts.md](shortcusts.md)
