# Phase 1 — Application shell and deterministic state core: checklist

Status: COMPLETE (2026-09-02)

## Scope delivered (plan §19 Phase 1, kata issue 3s1p)

- `src/runtime/terminal.rs` — raw mode + alternate screen, idempotent
  `restore()`, panic hook that restores *before* the previous hook prints
- `src/runtime/events.rs` — Crossterm `EventStream` + 250 ms ticks on a
  tokio mpsc channel; press-only key filtering; resize forwarding
- `src/runtime/logging.rs` — `tracing` file subscriber (daily rolling,
  non-blocking, `RUST_LOG`-filterable); never writes into the TUI
- `src/app/action.rs` — plan §9 action vocabulary (two documented
  deviations: `BackendCompleted` deferred to Phase 3; `SearchEdit` added
  for search-field text editing, which §9 lacks)
- `src/app/focus.rs` — `SearchField | Sidebar | MessageList`, Tab order,
  shortcut gating in text fields
- `src/app/route.rs` — route stack; Phase 1 constructs `Route::Mailbox`
  only (reader/composer/search extend it in their phases)
- `src/app/state.rs` — `AppState` subset (operations registry + overlays
  arrive in Phase 3, config in its own work)
- `src/app/reducer.rs` — deterministic, I/O-free `reduce(&mut AppState,
  &Action)`; mock page loads are pure and become effects in Phase 2/3
- `src/app/mock.rs` — mock mailboxes + 25 inbox messages (2 pages) from
  `mockups/list.html`, fixed timestamps (`now()` = 2026-09-02 10:47 +03)
- `src/ui/theme.rs` — semantic tokens (plan §18) approximating the mockup
  oklch palette; no literal colors outside the module
- `src/ui/layout.rs` — full ≥120×24, compact ≥90×20, too-small otherwise
- `src/ui/components/{topbar,sidebar,statusbar}.rs`, `screens/mailbox.rs`
  per `mockups/list.html`; no labels block, no storage meter, no snoozed/
  scheduled/quota elements (plan §3/§4)
- `src/ui/{dates,text}.rs` — centralized date formatting; Unicode-safe
  width-aware truncation
- `src/input/keyboard.rs` — plan §10 input contract, pure `KeyEvent →
  Option<Action>`
- `src/main.rs` — tokio current-thread runtime, event loop
  (draw → poll → translate → reduce), `POST_INDUCE_PANIC=1` test hook
- Supporting domain types (`domain/{address,mailbox,message,page}.rs`)
  added now because state requires them; Phase 2 extends, not rewrites

## Acceptance criteria (plan §19 Phase 1)

- [x] App opens and exits cleanly with `Esc`/quit action — pty smoke:
      Esc → exit 0; `?1049h`…`?1049l` pair in capture; Ctrl+C → exit 0
- [x] Terminal restores after Ctrl+C and induced panic test —
      `POST_INDUCE_PANIC=1` under a pty: exit 101, `?1049l` emitted by the
      panic hook before the readable panic report; restore() is idempotent
      (unit-tested); Ctrl+C path exits cleanly (above)
- [x] Reducer is I/O-free and unit tested — 20 reducer tests: movement
      clamps, empty list, page boundaries (no invalid requests), selection
      identity across refresh, route restoration semantics, focus cycling,
      Esc ordering, resize, tick purity, no-op vocabulary
- [x] Mock mailbox screen renders at full, compact, and too-small sizes —
      `tests/terminal_snapshots.rs` on `TestBackend`: 152×40 (sidebar,
      snippet, range), 100×30 (sidebar hidden, rows clip like the mockup's
      compact 14ch column), 60×15 (message only, no chrome); selected-row
      accent fill and unread bold asserted; resize through actions
      re-renders into the right mode
- [x] No `j`/`k` or help shortcut is shown — keyboard tests assert `j`,
      `k`, `?` map to nothing; snapshot tests assert no "help"/"j k"/"j/k"
      and no out-of-scope elements (Labels, Snoozed, quota) in any frame

## Commands run / results

- `cargo fmt --check` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --all-targets --all-features` — 55 passed, 0 failed
  (49 unit + 6 render)
- pty smoke via `script -q` (TERM=xterm-256color):
  - Esc: exit 0, clean restore
  - Ctrl+C (byte sent 1 s after start, i.e. after raw mode): exit 0
  - `POST_INDUCE_PANIC=1`: exit 101, terminal restored pre-report
- Log verified at `$TMPDIR/tmail/log/tmail.log.2026-09-02` (file only)

## Notes for Phase 2+

- Render pipeline draws every event (incl. ticks); fine at Phase 1 cost,
  revisit if resize storms show up.
- `AppState.size` initializes from `crossterm::terminal::size()`; a pty of
  0×0 degrades to the too-small message correctly.
- Ctrl+C must be sent after raw mode is active in scripted tests —
  canonical-mode ISIG otherwise swallows it (harness artifact, not app).
- Mock page loads are inline calls today; the reducer's state transitions
  are written so Phase 3's effect system can swap `mock::mock_page` for
  backend effects without touching the transition logic.
- Search field: editing/focus/Esc work; submit is a debug no-op until
  Phase 9 (plan §16).
