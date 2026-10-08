# Tmail Code Review

- **Date:** 2026-10-07
- **Revision reviewed:** `master` @ `2179b8f` (version 0.8.6), working tree clean
- **Scope:** all of `src/` (~51 kLOC) and the first-party integration tests in `tests/`
- **Method:** manual reading of the highest-risk paths (process spawning, config/secret handling, draft/send lifecycle, attachment save, event loop), plus parallel subsystem reviews. Every High/Critical finding below was re-verified against the source and, where the behavior lives in a dependency, against the dependency's source (himalaya 2.1.0/2.2.1, io-smtp 0.3.0/0.5.0, mail-builder 0.5.0, crossterm 0.29.0, toml 0.9.12).
- **Conventions:** severity is assigned for the documented/supported configuration (macOS/Linux primary, himalaya v2.x, IMAP+SMTP). Long-standing issues in documented limitations are not re-litigated.

## Severity summary

| # | Severity | Area | Finding |
|---|----------|------|---------|
| 22 | Medium | app/state | Previews/saved attachments keyed by `MessageId` alone — cross-mailbox collisions |
| 23 | Medium | wizard | Stale discovery results apply off-screen; `e`/`r` are not gated while discovering |
| 24 | Low | wizard | Repeated `Enter` on Confirm starts concurrent account saves (no busy gate) |
| 25 | Low | app/overlays | An error modal opened over another overlay restores focus to a vanished overlay |
| 26 | Low | input/keymap | Restoring structural defaults can leave one key bound to two actions; help text lies |
| 27 | Low | input/mouse | Mouse drag release activates a click despite the documented contract |
| 28 | Low | runtime/tasks | Raw URL logged unsanitized at debug (`tasks.rs:494`) |
| 29 | Low | main/signals | Signals are deferred while the external editor runs (Ctrl-Z in the editor suspends on return) |
| 30 | Low | view/ui | `u16` overflow in overlay math at absurd terminal widths (debug panic) |
| 31 | Low | backend/map | `tz_hour.abs()` / minute math can panic or wrap on malformed backend date fields |
| 32 | Low | domain/draft | `start_save`'s overflow fallback overflows eagerly exactly when it is used |
| 33 | Low | domain/draft | `confirm_saved` overwrites `remote_id` from a stale confirmation |
| 34 | Low | backend/journal | Account scope name `".."` escapes the journal root |
| 35 | Low | domain/paths | `expand_tilde` corrupts non-UTF-8 paths |
| 36 | Low | config/write | A failed fresh-file save can leave a partial config on disk |
| 37 | Low | config/write | `DraftAccount.name` with `.` panics; names/aliases are interpolated unescaped into TOML |
| 38 | Low | wizard/confirm | "Default: yes" preview contradicts the write when the collision suffix is chosen |
| 39 | Low | config | Several startup issues name non-existent config keys |
| 40 | Low | config | `~user` passes `downloads_dir` validation but is never expanded |
| 41 | Low | discovery | Timeout message says 17 s; the wrapper times out at 16 s |
| 42 | Low | discovery | `Provider::from_host` matches substrings (`notgmail.com` → Gmail) |
| 43 | Low | wizard | "edit it (e)" hint points at a screen where `e` just types a character |
| 44 | Low | runtime/tasks | Save-flow comment contradicts behavior: provisioning errors abort the save |
| 45 | Low | backend/map | Alias role resolution is nondeterministic when two roles point at one mailbox |
| 46 | Low | app/page_cache | Temp files outside `messages/` are never swept |
| 47 | Low | app/reader | Reader-document cache fingerprint is length-only |
| 48 | Low | app/state | `saved_attachments` is unbounded |
| 49 | Low | backend/process | The normal wait branch can block on inherited pipes past the timeout budget |
| 50 | Low | app/cache | A late page-store can resurrect a page an evict just removed |
| 51 | Low | domain/private_fs | Private FS helpers follow symlinks (`O_NOFOLLOW`/`create_new` not used) |

---

## Detailed findings

### 22. Medium — Session caches keyed by `MessageId` only

**Location:** `src/app/state.rs:231,246,250`; `src/app/reducer/message_results.rs:526-533,570-584`

`previews`, `preview_requested`, and `saved_attachments` use bare `MessageId` keys, while backend ids are per-mailbox (`MessageLocator` always pairs mailbox+id). `cancel_previews` only cancels `MailOperation::Preview`, leaving `CacheOperation::CachePreviewLoad` reads alive across `switch_mailbox`, and completion looks up rows by `s.id == locator.id` without comparing `mailbox_id`. Same-id rows in a different mailbox can receive another mailbox's snippet/attachment path. The cache test only registers `MailOperation::Preview`, missing the cache stage.

### 23. Medium — Stale discovery result / ungated override keys

**Location:** `src/app/wizard.rs:992-995,1037-1064`; key handling `:541-568`

`cancel_wizard`'s `Discovery → Email` arm does not cancel `in_flight`/clear `discovering`, and `wizard_completed` checks only the operation id, not the current step. A late `Err` paints an error on the email screen; an empty `Ok` calls `open_override` off-screen. `submit_email` never clears `override_open`/`override_fields`, so a later submission can show stale override fields. Also `e`/`r` are not gated on `discovering`, so override fields can be edited invisibly behind the spinner and `r` can spawn unbounded blocking discovery workers (which ignore cancellation and run to their 15 s deadline).

### 24. Low — Concurrent wizard saves

`ConfirmSave` has no `in_flight`/busy gate and `AccountOperation::supersedes` returns false, so a second Enter starts a concurrent read-modify-write merge; the second `in_flight` overwrite hides the first completion, and Esc then cancels only the second. `WizardState::is_busy()` exists but has no callers.

### 25. Low — Focus restored to a vanished overlay

`open_error_modal` records `previous_focus: state.session.focus` (`modals.rs:867`). When a foreground failure lands while help/chooser/switcher/mailboxes is open (these let results fall through), the error modal opens over it, and Dismiss restores focus to the overlay's `Focus` variant even though the overlay is gone. Tab self-cycles; `Esc` can quit at the root.

### 26. Low — Double-bound key after default restoration

`set_action` inserts specs into `by_key` without evicting them from other actions (`input/keymap.rs:191-209`), and `enforce_structural` uses it to restore structural defaults (`:332-358`). A config that releases a structural key and binds it to another action (`cancel = []`, `trash = ["Esc"]`) ends with Esc cancelling while help/status still advertise "Trash Esc".

### 27. Low — Mouse drag release activates

`input/mouse.rs:89-95` treats every left `Up` as a click and hit-tests the release position. No `Down` state is tracked despite the comment promising drags never activate.

### 28. Low — Raw URL logged

`runtime/tasks.rs:494` logs `url = %url` for `OpenUrl` at debug without `sanitize`, bypassing the project's own rule (`domain/sanitize.rs` exists precisely to redact URL userinfo).

### 29. Low — Signals deferred during the editor

The signal arm is only polled by `run_event_loop` (`main.rs:591-636`), while `handle_effects` awaits the editor inline. A Ctrl-Z in the editor is caught by tmail's `SIGTSTP` registration and only acted on after the editor exits.

### 30. Low — `u16` overlay overflow

`(size.0 * 3 / 4)` (`view/overlay.rs:242,254`, `ui/components/attachment_dialog.rs:24`) overflows for widths > 21845 (settable with `stty cols 30000`); debug panic, wrong layout in release.

### 31-35. Low — Backend/domain robustness

- `backend/himalaya/map.rs:239`: `raw.tz_hour.abs() * 60 + raw.tz_minute as i32` panics on `i32::MIN` (debug) and wraps for large fields; `tz_minute as i32` also wraps. The function promises "never a panic".
- `domain/draft.rs:206-208`: `unwrap_or(now.timestamp_millis() * 1_000_000)` evaluates eagerly and overflows exactly for the out-of-nanos-range dates where `timestamp_nanos_opt()` is `None`.
- `domain/draft.rs:226-232`: `confirm_saved` guards `saved_revision` against stale confirmations but unconditionally overwrites `remote_id`, so an old confirmation can point at a copy a newer save deleted.
- `backend/journal.rs:137-150,222-237`: the scope sanitizer preserves `.`, so an account named `..` escapes the drafts root; config/CLI-controlled, but it defeats the sanitizer's stated purpose.
- `domain/paths.rs:101-108`: `to_string_lossy()` + `home.join(rest)` replaces non-UTF-8 bytes with U+FFFD, pointing at a different file.

### 36-44. Low — Config/wizard edge cases

- `config/write.rs:344-360`: the fresh-file branch leaves a partial file on write failure (the atomic branch cleans up), contradicting "On failure nothing partial is left behind".
- `config/write.rs:239-248,282-331`: `DraftAccount.name` containing `.` makes `doc["accounts"][&name]` miss and panic; name and alias keys are interpolated into a TOML fragment unescaped. Unreachable through the wizard (names are sanitized) but a public-API panic/injection.
- `app/wizard.rs:366-371,934-938`: "Default: yes" is computed from the unsuffixed name, but the writer's default decision uses the suffixed final name — the preview can contradict the file.
- `config/mod.rs:637,688,693,718,723,732,735,745,997`: issues name keys like `[tmail.mail].mail.page_size_auto` and `[tmail.composer].composer.editor`, which do not exist.
- `config/mod.rs:1168-1177`: `downloads_dir` accepts any leading `~` (including `~user`), but `expand_tilde` only handles `~`/`~/`.
- `discovery/mod.rs:198,234`: the error says "17s deadline" while the timeout is `15 + 1 = 16s`.
- `discovery/mod.rs:103-115`: `host.contains("gmail.com")` tags `notgmail.com` and `imap.gmail.com.evil.example` as Gmail (same for outlook/hotmail/live/office365).
- `app/wizard.rs:908-913`: "edit it (e)" is shown on W4, where `e` types a character; the instruction should point back to the server list.
- `runtime/tasks.rs:569-575`: the comment says provisioning failures degrade to "none created", but `operation_failure` aborts the save for every non-cancelled error.
- `backend/himalaya/map.rs:339-346`: role resolution iterates a `HashMap`, so when two alias keys map to the same mailbox the resolved role varies between runs.

### 45-51. Low — Caches, files, and concurrency

- `app/page_cache.rs:342,494-513`: temp files under `<root>/<mailbox>/` and `mailboxes.json.tmp-*` are never swept (only `messages/` is).
- `app/reader.rs:383-387`: the cached reader document is keyed on `(html.len(), plain.len(), attachment count)`; a same-length body swap serves stale lines.
- `app/state.rs:250`: `saved_attachments` grows without bound, unlike `previews` (capped) and `preview_requested`.
- `backend/himalaya/process.rs:208-233`: the normal wait branch awaits stdin/reader tasks unbounded; a grandchild holding a pipe after the direct child exits can stall past the 30 s budget (the timeout arm is no longer selectable).
- `app/operation/cache.rs:94-159`, `app/reducer/results.rs:674-685`: page-store and page-evict never supersede each other, so a late store can resurrect a page an evict removed.
- `domain/private_fs.rs:82-99,113-120`: `open()` truncates and `read()` chmods through symlinks; `O_NOFOLLOW`/`create_new` are not used. If `TMAIL_DATA_DIR` points into a shared directory, a planted symlink can redirect those operations.
- `config/write.rs:371-410`: (see finding 19 for the symlink replacement issue; the temp-file cleanup itself is correct).

---

## Test-quality observations

- `reducer_tests/cache.rs:926` registers only `MailOperation::Preview` when testing switch cancellation, missing the `CachePreviewLoad` stage that survives a mailbox switch (finding 22).- `reducer_tests/composer.rs:65` codifies the composer overwrite on mailbox switch without asserting the parked draft was secured (finding 15).
- No test drives a foreground failure arriving while help/chooser/switcher/mailboxes is open (finding 25) or `Tick` during the wizard (finding 17).
- Attachment tests never move the reader focus between `SaveAttachment` start and completion (finding 13).
- The suite otherwise shows unusually good discipline: process cancellation/timeouts, sanitization, private-FS modes, journal atomicity, keymap parsing, and reducer state machines all have direct tests. The gaps above cluster exactly where two subsystems interact (overlay vs. result routing; composer identity vs. send completion).

## Areas reviewed with no findings

- `runtime/terminal.rs` (panic-safe/idempotent restoration, balanced title stack), `runtime/editor.rs` (argv-only spawn, 0600 temp file), `runtime/signals.rs` (suspend/resume correctness aside from the editor deferral).
- `runtime/events.rs` coalescing rules (wheel runs, key-cancels-scroll, tick collapse, modal filtering) and `input/keyspec.rs` parser/round-trip.
- `input/keyboard.rs` focus routing and Ctrl+Enter handling; hit-test smallest-region/modal isolation in `input/mouse.rs`.
- `view/dates.rs`, `view/layout.rs`, `view/theme.rs`, `view/rich.rs`, `view/text.rs` (saturating/clamped math; Unicode-safe clipping; no remote fetches).
- `ui/chrome.rs`, `ui/components/*`, `ui/screens/*`: no panics on degenerate rects, masked secrets never rendered.
- `backend/himalaya/command.rs` (argv-only, no shell), `dto.rs` (lenient parsing), `process.rs` kill/cancel/reap logic (`unsafe libc::kill(-pid)` is sound for owned process groups).
- `backend/private_fs` mode repair (aside from the symlink note), `journal.rs` per-draft locking/atomic writes (aside from findings 34/35), `domain/sanitize.rs` pattern coverage for its documented scope, `domain/send.rs`/`reply.rs` validation, `app/state.rs` title sanitization, `app/page_cache.rs` path safety, `OperationRegistry` supersession/coalescing semantics, `wizard.rs` secret `Debug` redaction and text-field bounds.

## Suggested priority

2. Findings 12–23 — behavioral bugs clustered around composer/draft/cache lifecycles; each needs a regression test.
5. Remaining Low items — batch fixes with the surrounding code.
