# Architecture

One page: the layers and the data flow, so a reader knows where to look.
Every module carries its own detailed documentation in `//!` headers; this
map is the level above source code. Test suites live in `tests/` (contract,
integration, snapshots, pty smoke — see [development.md](development.md)).

## The loop

Elm-style: the reducer computes state transitions and never touches I/O.
Effects and async results flow around it as data.

```mermaid
digraph g {
  rankdir=LR;
  events [label="runtime::events\n(keys, mouse, tick, resize)"];
  results [label="runtime::tasks\n(OperationManager)\nchannel"];
  reducer [label="app::reducer\n(I/O-free)"];
  state [label="app::state\nAppState"];
  manager [label="OperationManager\nspawn+cancel per Effect"];
  backend [label="backend::MailBackend\n(trait)"];
  frame [label="ui::render"];
  events -> reducer [label="Action"];
  results -> reducer [label="Action::BackendCompleted\n/ EditorFinished"];
  reducer -> state [label="mutations"];
  reducer -> manager [label="Effect"];
  manager -> backend [label="argv-only child\n+ CancellationToken"];
  accessops [label="external editor\n(runtime::editor)\nplatform open\n(backend::opener)\ndiscovery\n(discovery)"];
  manager -> accessops;
  state -> frame [label="read"];
  backend -> results [label="OperationResult"];
}
```

1. **`src/main.rs`** — wiring and the event loop only: build config →
   keymap → backend/discoverer/opener (all injected as trait objects) →
   `OperationManager`. Each event runs
   `reduce(&mut state, &action)`; the returned `Vec<Effect>` is routed
   here (backend effects to the manager, the external editor runs inline
   on the terminal owner, plan §14). One draw per drained event batch.

2. **`app::reducer`** — pure state transition, returns effects. One
   function per concern (modal slices, selection, pagination, drafts,
   wizard); `backend_completed` maps each `OperationKind`'s result back
   into state. All knowledge of *what to do* lives here; *how to do it*
   never does.

3. **`app::operation`** — `OperationKind` (typed intent),
   `OperationRegistry` (ids, cancellation tokens, supersession), the
   result payloads (`OperationOutcome`).

4. **`runtime::tasks` (`OperationManager`)** — spawns one cancellable
   task per `Effect` against the injected backend, sanitizes failures
   into `OperationFailure` (plan §12), and feeds results back through an
   unbounded channel that re-enters the reducer as
   `Action::BackendCompleted`. Cancellation is the cancellation token's
   job (kills the child process).

5. **`backend/`** — `traits.rs` defines `MailBackend`
   (all mail operations), `PathOpener` (platform open-with), and the
   error taxonomy. The only implementation is
   `backend::himalaya` (ADR 0001): argv building (`command.rs`), the
   argv-only child process with cancellation (`process.rs`), JSON DTO →
   domain mapping (`map.rs`), and the crash-safe draft journal
   (`journal.rs`, ADR 0002). Tests drive a fake himalaya binary.

6. **`domain/`** — transport-independent types both sides of the
   backend trait speak (`Message`, `MessageSummary`, `Mailbox`, `Page`,
   `Draft`, `MessageLocator`, addresses), plus the OS-facing helper
   modules the layers share: `paths.rs` (path expansion, executability)
   and `private_fs.rs` (owner-only storage). No UI.

## The slices

- **`ui/`** — state → frame: `mod.rs` is the render entry, `screens/`
  (mailbox, reader, composer, wizard) and `components/` (chrome, dialogs,
  sidebar, bars) draw it, `chrome.rs` holds shared helpers. Mouse clicks
  translate through rendered `HitMap`s (`input/mouse.rs`). Rendering does
  not mutate application state, with one documented exception: the reader
  memoizes its built document in `AppState::caches.reader_doc`
  (`app/reader.rs::scroll_document`, a `RefCell` — see `app/state.rs`) so
  frames do not re-parse the body.

- **`view/`** — the shared view model: pure rendering-adjacent computation
  imported by both `app` (reducer scroll/page bookkeeping) and `ui`
  (drawing). `layout.rs` owns the responsive geometry
  (full/compact/too-small) that the reducer also consults, so scroll
  clamps and what is drawn can never disagree; `theme.rs` (palettes +
  runtime switching), `rich.rs` (HTML → spans, plan §13), `text.rs`,
  `dates.rs`, `overlay.rs`.

- **`input/`** — translation from key/mouse events to `Action`s.
  `keymap.rs` is the single source of bindings (`[tmail.keybindings]` +
  defaults), `keyspec.rs` the spec grammar, `keyboard.rs` the
  focus-dependent disambiguation (text fields, wizard, modals).

- **`config/`** — the one shared TOML file (`[tmail]` tables) next to
  himalaya's `[accounts.*]` (which Tmail never rewrites). `parse_with_issues`
  collects all problems for reporting at startup; every config knob is in
  the README's tables. `write.rs` is the wizard's format-preserving
  account merge (ADR 0003 §3.6). `list_accounts` enumerates the
  `[accounts.*]` tables for the runtime account switcher, whose confirmed
  selection re-loads the file through `Config::load_with_account_override`
  (ticket c0n0).

- **`discovery/`** — the account-setup wizard's email-settings discovery
  (providers, server URLs, security) wrapping `io-pim-discovery`;
  `TMAIL_FAKE_DISCOVERY=1` injects a canned fake.

## Cross-cutting rules

- **I/O never enters the reducer.** File I/O lives in the wizard's
  operations, the draft journal, and the page cache (`app/page_cache.rs`).
- **Sanitization before any user-visible or logged string**
  (`domain/sanitize.rs`, plan §12); secrets never reach logs or UI
  (`config/write.rs::SecretStorage`).
- **Terminal lifecycle stays in `runtime/terminal.rs`** (raw mode,
  alternate screen, panic-safe restore, editor suspend/reenter).
- The external editor is the one synchronous effect: allowed to run only
  on the thread owning the terminal (plan §14).
