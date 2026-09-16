 
## Keyboard

Every shortcut below is the *default*: actions can be reassigned, extended,
or unbound per context via `[tmail.keybindings.<context>]` in the config
(see `config.example.toml` for the full annotated list). The status bar
hints always follow the configured bindings.

| Context | Key | Action |
| --- | --- | --- |
| Global | `↑` / `↓` | Move selection / scroll focused area |
| Global | `←` / `→` | Previous / next page (reader: page the viewport) |
| Global | `Enter` | Open / activate focused control |
| Global | `Esc` (or `q`) | Cancel work → clear selection → close overlay → go back |
| Global | `Tab` / `Shift+Tab` | Next / previous focus |
| Global | `/` | Focus search |
| Global | `c` | Compose |
| Global | `Ctrl+R` | Manual refresh |
| Global | `Ctrl+C` | Quit |
| Message list | `Ctrl+A` | Select all visible messages (or clear the selection) — mail-list key, not global |
| Global | `m` | Toggle mouse capture on/off (see [Mouse](#mouse)) |
| Global | `t` | Cycle the theme: built-ins first, then `[tmail.themes.<name>]` |
| Global | `Ctrl+G` | Account switcher: restart into another `[accounts.<name>]` (confirm first when work is in flight or the composer has unsaved edits) |
| Global | `?` (in text fields: `Ctrl+H`) | Shortcuts popup over the current screen (wizard excluded) |
| List/reader | `r` / `a` / `f` | Reply / reply-all / forward. From the reader they seed from the open message; from the list the message is loaded first¹, then the composer opens |
| List/reader | `e` / `s` / `u` | Archive / star / mark unread |
| List/reader | `Delete` / `d` | Trash |
| Reader | `⌫` (Backspace) | Trash |
| List | `Space` | Toggle the focused message's selection mark |
| List | `i` | Mark read (focused row, or the whole selection) |
| Reader | `Tab` / `Shift+Tab` | Cycle the open message's links, then its attachments |
| Reader | `Enter` | Open the focused link in the browser, or the focused attachment |
| Reader | `S` / `o` | Save / open the focused attachment (first chip when none is focused) |
| Search | printable, `Backspace`, `Enter`, `Esc` | Edit query, submit, leave |
| Composer | `Enter` | Newline in body; activate focused control |
| Composer | `Ctrl+Enter` | Send |
| Composer | `Ctrl+E` | Edit the body in the configured external editor |
| Composer | `Esc` | Save and leave (never silently discards) |
| Modal | `↑↓` / `Tab` / `Enter` / `Esc` | Scroll, switch button, confirm, dismiss/keep |
