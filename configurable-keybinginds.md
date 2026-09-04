# Configurable keybindings — research (ticket 00y7)

Question: how hard is it to make Post's keyboard shortcuts configurable
from the config file? This document describes how it would work, the
options, the difficulties, and the decisions that need to be made before
implementation.

Two motivating examples from the ticket:

1. add `j` / `k` in normal mode to move the cursor up / down in the
   message list;
2. rebind the delete action so `backspace` acts like `Esc`.

Both are action-level rebinds ("run action X when key K is pressed"),
which is exactly the layer the current code already isolates well.

## Current architecture (what makes this easy or hard)

The keyboard pipeline has three layers, and only the middle one would
change:

1. **Event source** — crossterm's event stream (`src/runtime/events.rs`)
   delivers `KeyEvent { code: KeyCode, modifiers: KeyModifiers }`.
   Nothing here needs to change.
2. **Translation** — `keyboard::to_action(key, focus) -> Option<Action>`
   (`src/input/keyboard.rs:13`): a pure, hardcoded `match`. This is the
   layer a config would feed. Its invariants:
   - Text-entry foci (`SearchField`, `Dialog`, `Composer`) consume every
     printable character as text and never fire single-letter shortcuts
     (`keyboard.rs:82-101`). This must survive any configuration.
   - Structural keys (`Esc`, `Enter`, `Tab`/`BackTab`, arrows) have
     roles everywhere (cancel / activate / focus / move).
   - Some keys are focus-gated: `space` (list only), `d` (list+reader
     trash, reader attachment save on `S` — ticket zg41).
3. **Actions** — `Action` enum (`src/app/action.rs`) handled by the
   reducer. Unchanged: configuration rebinds keys to *existing*
   actions; it does not define new behavior.

Other touchpoints that hardcoded bindings currently reach:

- **Status bar hints** are per-screen hardcoded tables
  (`src/ui/components/statusbar.rs:105-132`). If a user rebinds `d`,
  the hint "d delete" lies unless hints read the binding table.
- **Mouse click targets** are independent of the keyboard (each
  `ClickTarget` maps to an action directly), so rebinding keys never
  breaks the mouse.
- **Docs** (`docs/shortcusts.md`) document the defaults and will drift.

Precedents in the codebase that a keybinding config should copy:

- **Theme overrides** (`[post.theme]`): a fixed token list, parsed
  values validated at load, applied over defaults in file order, unknown
  keys reported (`src/config/mod.rs:592-632`). The closest model: named
  "tokens" (actions) + string values (key specs) + `LoadIssues` report.
- **Forgiving-with-issues loading**: an invalid value never crashes the
  app; it falls back to the default and the problem is reported at
  startup (`parse_with_issues`, `src/config/mod.rs:281`).
- The config file is shared with himalaya; `[post]` is already a
  tolerated-unknown table for himalaya, so `[post.keys]` needs no
  coordination.

## Options

### Option A — one override table, action names → key specs (recommended)

```toml
[post.keys]
move_up = "k"
move_down = "j"
back = "backspace"        # example 2: backspace behaves like Esc
```

- A static registry maps action names (`move_up`, `compose`, `reply`,
  …) to their default keys — essentially the current `match` arms
  written as data.
- Parsing produces a `KeyMap` (action → key, plus reverse index key →
  action per context), built once at startup and stored on
  `AppState` (like `Theme`).
- `to_action` becomes `to_action(keymap, key, focus)`; the body looks
  up the reverse index, gated by focus exactly as today.
- Unknown action names, unparseable key specs, and conflicts are
  reported via the existing `LoadIssues` channel; defaults fill the
  gaps.

Effort: **small-medium** (≈1–2 days): registry + parser + `to_action`
refactor + hint rendering from the table + tests. No event-loop,
reducer, or backend changes.

### Option B — per-context tables (follow-up to A)

```toml
[post.keys.global]   # everywhere shortcuts fire
[post.keys.list]     # MessageList + Sidebar
[post.keys.reader]
```

More power, more surface: the contexts must map onto `Focus`, and every
lookup becomes two-level (context then action). Worth it only if users
actually need the same key to mean different things per screen. The
focus gating that already exists in `to_action` (e.g. `space` list-only)
covers most real needs without per-context tables.

Effort: +≈1 day over A.

### Option C — sequences and modes (vim-style `gg`, `g i`)

Requires a chord/sequence state machine in the event loop (buffer keys,
timeout, prefix matching), plus a mode concept. This is a different
scale of change and a poor fit for v1; the single-key grammar covers the
stated use cases.

Effort: large; not recommended now.

## Key spec grammar (needed for any option)

Value strings must be parsed into `KeyCode` + `KeyModifiers`:

- printable characters: `"j"`, `"?"`, `"!"`;
- named keys: `backspace`, `delete`, `esc`, `enter`, `tab`, `home`,
  `end`, `pageup`, `pagedown`, `f1`–`f12` (everything crossterm
  distinguishes and Post might bind);
- modifiers as prefixes, `+`-joined: `ctrl`, `alt`, `shift`
  (`"ctrl+r"` — note `ctrl+r` refresh already exists as a hardcoded
  chord, so chords should be representable even if rarely used);
- case rule to decide: a lowercase letter is the plain character;
  an uppercase letter (`"S"`) means `shift` + letter. Today `s` (star)
  and `S` (save attachment) are distinct bindings and the distinction
  is load-bearing (zg41), so the grammar must keep it explicit.

The parser is small and table-testable; the theme-hex parser
(`parse_hex_color`) is the model for its error style.

## Difficulties and risks

1. **Conflicts.** Two actions bound to the same key (user binds `j`,
   which is free today — fine; user binds `r`, shadowing reply). The
   loader must detect: (a) the same key bound to two actions within one
   scope, (b) a rebind that shadows a default binding of a *different*
   action. Policy options: refuse (error issue, defaults kept), or
   last-wins with a warning. Recommend: last-wins + warning, consistent
   with theme overrides applying last occurrence.
2. **Lying hints.** The status bar hint tables are hardcoded. If hints
   do not read the `KeyMap`, customization makes the UI lie. Either
   derive hint text from the table (the hint labels "reply", "archive"
   become lookup-by-action with a fallback to hiding the hint) or
   declare hints best-effort. Recommend: derive from the table.
3. **Text-entry invariant.** No binding may leak into
   `SearchField`/`Dialog`/`Composer`. Keep the existing early-return
   gates *before* any table lookup; never make text-entry foci
   configurable (typing must type).
4. **Structural keys.** `Esc`, `Enter`, `Tab`, arrows participate in
   focus, activation, and cancel flows across all screens. Rebinding
   them invites dead-ends (e.g. no way to confirm a modal). Recommend:
   exclude them from the configurable registry in v1 (`backspace` is
   *not* structural — it is only an edit key in text foci, and as of
   zg41 trash in the reader — so example 2 is safe).
5. **Binding collisions with defaults over time.** The registry must be
   the single source of truth (the `match` arms, the docs, and ideally
   the tests derive from it), or three lists will drift. A
   `#[test]` that walks the registry and asserts each default is
   unique per scope keeps it honest; `keyboard_tests.rs` becomes
   table-driven over the defaults.
6. **The `q`/Esc mirror and focus-gated arms.** Several arms are not
   simple data (`q` mirrors `Esc` per focus; `space` list-only; `d`
   list+reader). The registry needs a scope per binding
   (`global` / `list` / `reader`) so these become data too — this is
   the one real refactor inside `to_action`.
7. **Case sensitivity on international layouts.** `crossterm` reports
   what the layout produces; shifted symbols (`?`, `!`) arrive as their
   char. Grammar should accept the char as-written and not over-engineer
   layout detection.

## Decisions to make before implementing

1. Scope: single override table (A) now, per-context (B) later — or
   straight to B?
2. Conflict policy: last-wins + warning (recommended) vs refuse.
3. Which keys are out of scope: recommend excluding `esc`, `enter`,
   `tab`, `backtab`, arrows, `ctrl+c`, `ctrl+q` from rebinding in v1.
4. Whether `j`/`k` should ship as *built-in* defaults (plan §4
   deliberately omits them) or only as a config possibility.
5. Whether hints render from the `KeyMap` (recommended) or stay static.
6. Config surface name: `[post.keys]` (recommended; mirrors
   `[post.theme]`).
7. Whether the reader `backspace` = trash default (zg41) stays — a
   configurable `back` would let users who prefer backspace-cancels map
   it themselves; the zg41 default should not silently change.

## Recommended plan (summary)

1. Introduce `KeyMap` (registry of action → scope + default key;
   reverse index), built from `[post.keys]` overrides + defaults,
   validated with `LoadIssues`.
2. Thread it through `AppState`; change `to_action` to consult it,
   keeping the text-entry gates and structural keys fixed.
3. Derive status-bar hints from the `KeyMap`.
4. Table-driven tests over the registry; docs note that
   `docs/shortcusts.md` lists defaults.
5. Ship A; add per-context tables (B) only on demand; sequences (C) are
   out of scope for now.

Total: roughly 1–2 focused days for Option A with tests and hint
support, no risk to the runtime event loop or the reducer.
