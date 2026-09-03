# Phase 9 — Search and refresh: checklist

Status: COMPLETE (2026-09-03)

## Scope delivered (plan §19 Phase 9, kata epic bvw9 + 6 children)

- **9.1 Search reuses the message list** — a submitted search pushes
  `Route::Search(SearchRoute { query, mailbox_id })` above the mailbox
  route and search results render through the same list page state,
  selection, scroll, and pagination the mailbox uses. The mailbox's list
  context (page, selection, scroll) is stashed in
  `AppState.search_return` (`ListStash`) when the search opens and is
  restored *exactly* on leaving — no reload, no selection drift. The
  reader opens on top of search results and returns to them untouched.
  Switching mailboxes from search rebuilds the route stack (the reducer
  now clears the whole stack, fixing a latent "reader above search" case)
  and drops the stash. Re-submitting edits the query in place without
  disturbing the stash; a new search supersedes the previous one (same
  mailbox).
- **9.2 Query passes unchanged** — `SearchRequest { mailbox_id, query,
  offset, limit }` carries the field text verbatim to a new backend trait
  method `search_messages`; no local parser, no rewriting. Verified
  against real himalaya 2.1.0 + Gmail IMAP: all flags precede the query
  because every trailing positional is parsed as the shared search DSL
  (`command.rs::envelope_search_argv` puts the query last, one argv
  entry, never a shell); an empty query is valid backend behavior
  (matches everything, so Post refuses submitting one with guidance
  instead); DSL errors surface as sanitized command failures.
- **9.3 Explicit search pagination** — results share the page machinery:
  Right/Left issue `Search` requests at ± page size; totals are not
  provided by the backend, so the page degrades to next-availability and
  the range label drops the "of N" part (plan §16). Empty results are
  valid: the list head names the query (`SEARCH — <query>`) and a dim
  `(no results)` note renders once the request landed (not while it is
  in flight).
- **9.4 Configurable timer + Ctrl+R** — `[post.mail].refresh_interval_seconds`
  (default 60, `0` disables, negative treated as 0) is parsed into
  `Config` and wired into the state at startup. The reducer arms the
  timer on the first injected-clock tick and, once the interval elapses,
  refreshes the *visible context* (mailbox page or open search) as a
  *background* operation. The timer stands down while a modal is open,
  the composer is on screen, or any operation is in flight, and retries
  on the next tick once they clear. `Ctrl+R` stays a foreground refresh
  and re-arms the timer so a manual refresh never collides with an
  imminent automatic one. Operation registry gained `OperationOrigin`
  (`start` vs `start_background`).
- **9.5 Selection/page preserved by Message-ID** — `apply_page` already
  re-resolves the selection by stable `Message-ID` first, then backend
  id (ADR 0001 finding 4); a new explicit test proves new mail arriving
  above the selection shifts rows but not the logical selection. Refresh
  targets only the visible context at its current offset; a foreground
  failure keeps the last coherent page with Retry/Dismiss.
- **9.6 Repeated auto-refresh errors suppressed** — a *background*
  refresh failure never opens the Retry/Dismiss modal: the first failure
  lands in the status line ("Refresh failed — the timer will retry") and
  the sanitized detail is recorded in `AppState.last_background_error`;
  repeated *identical* failures change nothing, and a success or a
  manual refresh clears the record. Foreground failures keep opening the
  modal, so manual refresh stays fully observable and available after
  auto-refresh failures (acceptance).

## Verification

- `cargo fmt --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features` (all suites green; new tests: search
  stash/submit/apply, reader round-trip from search, exact context
  restore, race-dropped results, re-submit editing, superseding,
  pagination requests, mailbox switch out of search, timer arm/fire,
  disable flag, stand-down on conflict/composer, search-context refresh,
  background failure suppression + record clearing, manual failure
  modal, selection preservation across new mail, registry origin flag,
  search argv exactness, refresh-interval config parsing, search render
  snapshot with query head and empty note).

## Notes for Phase 10+

- The search field keeps its text after leaving a search, so re-submitting
  is cheap; a future "clear" affordance can reuse `SearchEdit`.
- Search is per-mailbox (himalaya's shared API has no reliable
  account-wide search for IMAP; verified 2.1.0 help text). Account-wide
  search would need one request per mailbox or a backend change.
- Background auto-refresh currently also refreshes while the reader is
  open (the underlying list updates beneath it); the reader's data is
  unaffected. If that proves noisy, add a focus guard.
- `switch_mailbox` now clears the whole route stack; composer-route edge
  cases are unreachable today (sidebar activation requires list focus)
  but the invariant is now explicit.
- `OperationOrigin` rides on the registry entry; retries of background
  operations are foreground (user-initiated) and open the modal on
  failure — intentional.
