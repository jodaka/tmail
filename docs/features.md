# Features

## Account switching

With several `[accounts.<name>]` sections in the config file, `Ctrl+G` opens
the account switcher — a small list of every account, the one the session
drives marked `· current`. The account name also shows under the logo.

`Enter` on another account restarts Tmail with that account selected — the
functional equivalent of quitting and starting again with it: a fresh
config read from disk, empty mail state, per-account page cache and draft
journal. Enter on the current account just closes the popup.

Switching is safe by confirmation:

- **Work in flight** (a send, a refresh, a draft save…) — the dialog lists
  exactly what confirming cancels. An in-flight send may already be
  delivered; cancelling it leaves the outcome unknown, and the dialog says
  so in those terms before you confirm.
- **Unsaved composer edits** — listed too: confirming loses them (saved
  revisions stay in the account's Drafts mailbox and journal, and return
  when you switch back).

`Esc` aborts at any stage. If the reload after a confirmed switch fails
(the account vanished from the file, the file broke), Tmail falls back to
the configured default account instead of exiting, and reports why.

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
(least-recently-used eviction; see [Configuration](configuration.md)). For
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
- See [Known limitations](limitations.md) for backend-specific search
  caveats (non-ASCII text on IMAP, text search on Maildir).

## Mouse

Mouse support is **off by default** — enable it with `[tmail] mouse = true`
(see [Configuration](configuration.md)), or press `m` inside Tmail to toggle
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
