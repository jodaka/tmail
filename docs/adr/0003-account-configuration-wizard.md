# ADR 0003: Account configuration wizard

- **Status:** Accepted
- **Date:** 2026-09-08
- **Related:** ADR 0001 (himalaya backend boundary), `src/config/mod.rs` (shared one-file configuration)

## 1. Context

Tmail requires a working himalaya account block before it is useful, but
today the only way to get one is hand-editing
`~/.config/himalaya/config.toml` (or running himalaya's own wizard and
redirecting its stdout into the file). A new client sees a fatal
"configuration problems" screen and must read documentation to proceed.
We want a guided in-app flow:

1. the user enters their email address;
2. Tmail detects the proper IMAP/POP/SMTP settings with
   [io-pim-discovery](https://github.com/pimalaya/io-pim-discovery);
3. the user enters their password;
4. Tmail verifies the account, derives mailbox aliases, and saves the
   account into the default himalaya config location.

Target account shape (himalaya 2.x, verified against the real-config tests
in `src/config/mod.rs`):

```toml
[accounts.gmail]
default = true
email = "some-email@gmail.com"
display-name = "Vasya Pupkin"

imap.server = "imaps://imap.gmail.com:993"
imap.sasl.plain.username = "some-email@gmail.com"
imap.sasl.plain.password.raw = "****"

smtp.server = "smtps://smtp.gmail.com:465"
smtp.sasl.plain.username = "some-email@gmail.com"
smtp.sasl.plain.password.raw = "****"

mailbox.alias.inbox = "INBOX"
mailbox.alias.sent = "[Gmail]/Sent Mail"
mailbox.alias.drafts = "[Gmail]/Drafts"
mailbox.alias.trash = "[Gmail]/Trash"
mailbox.alias.archive = "[Gmail]/All Mail"
```

### Constraints

- Tmail is a keyboard-first ratatui TUI with an I/O-free reducer
  (`Action` → `Effect` → `OperationManager` → `Action::BackendCompleted`).
  Anything the wizard does must fit that pattern or run outside the event
  loop deliberately.
- Tmail shares **one** TOML file with himalaya (ADR 0001 finding 13) and
  never touches existing himalaya blocks except the account it writes.
- himalaya 2.x has **no POP3 backend** (IMAP, JMAP, Maildir, Gmail REST,
  Microsoft Graph, SMTP). Discovery results for POP are therefore
  unusable in v1.
- Tmail drives exactly one account (the resolved `[tmail].account` or the
  default-flagged/sole account).
- `io-pim-discovery` is pre-1.0 (0.7.0 at the time of writing); its API
  will break. Pin the exact minor version in `Cargo.toml`.
- Tests and CI must never touch the network or a real himalaya binary.

### Decisions confirmed with the product owner

| Question | Decision |
| --- | --- |
| Wizard surface | **In-app TUI wizard** (ratatui screens, reusing existing modal/overlay components) |
| Discovery integration | **Rust library** (`io-pim-discovery` crate, in-process) — not the `pim-discovery` CLI |
| Trigger | **First-run auto** (no usable account) **+ manual entry point** |
| Password storage | **User chooses**: store `password.raw`, or reference a `password.cmd` command |
| Verification | **Test login before saving** + **auto mailbox-alias detection** |
| Existing config | **Merge** the new account into the existing file (format-preserving) |

## 2. Decision

Tmail ships a first-run **in-app TUI configuration wizard** that:

1. triggers automatically when startup finds no usable account, and is
   also reachable through a manual entry point (`tmail --configure`);
   It should be possible to gracefully exit from the app/wizard at any point with Ctrl+C 

2. discovers IMAP/SMTP settings for the entered address in-process via
   the `io-pim-discovery` crate (blocking client on a worker thread,
   bounded by a deadline), offering a **manual-override fallback** when
   discovery finds nothing;
3. lets the user choose between a stored raw password and a
   password-retrieval command, exactly mirroring himalaya's
   `password.raw` / `password.cmd` pair;
4. validates the draft account with a real himalaya `mailbox list` run
   against a **temporary 0600 config file** — nothing containing the
   credential is written to the real config until the test passes;
5. derives `mailbox.alias.*` from the tested listing (Gmail known-provider
   preset + generic name heuristics, filtered by what actually exists);
6. merges the account into the resolved config file with a
   format-preserving `toml_edit` edit, creating the file (mode 0600) if
   it does not exist.

POP discovery results are discarded (no himalaya backend); JMAP discovery
is recorded as future work.

## 3. Detailed design

### 3.1 Entry points and triggering

**First-run auto.** After `Config::load_with_issues` in `run()`
(`src/main.rs`), trigger the wizard when the resolved config yields no
drivable account:

- `config.path` is `None` (no file found anywhere), **or**
- the file exists but the `[accounts]` table is missing or empty.

A file that exists with accounts is never hijacked — even the
multi-accounts-without-`default` case keeps today's behavior (startup
proceeds with role resolution off; the startup issues list explains it).
The wizard replaces the first `Action::Refresh`/`LoadDrafts` warmup:
while the wizard route is active the reducer suppresses mailbox warmup
and the event loop runs unchanged.

**Manual.** `tmail --configure [path]` starts the same wizard regardless
of config state. On completion it prints the saved path to stdout and
exits 0; on `Esc`-cancel it exits 1 with `tmail: configuration not
changed`. The flag must not collide with the existing positional config
path (`args().nth(1)`): any first argument starting with `-` that is not
recognized is a hard usage error (v1 adds exactly `--configure`).

**Save target.** Reuse the resolution order already implemented in
`src/config/mod.rs::resolve_path` (CLI arg → `TMAIL_CONFIG` → existing
well-known candidates), with one addition: when nothing resolves, the
wizard creates `~/.config/himalaya/config.toml` (first
`default_candidates()` entry even when it does not exist yet). A new
`config::default_save_path()` exposes that. The wizard always shows the
resolved target path on its summary screen.

### 3.2 Wizard flow (state machine)

One wizard route owns a small step machine. `Esc` always steps back;
from the first step it cancels the wizard. Steps:

```
W1 Email → W2 Discovery → W3 Identity → W4 Credentials → W5 Testing
                                                        │ ok        │ fail
                                                        ▼           └→ retry (W4)
W6 Aliases/Confirm → W7 Saved
```

- **W1 Email.** Single text field (reuse composer textarea components),
  input validation (non-empty, exactly one `@`, non-empty local part and
  domain — full RFC parsing is overkill). `Enter` → discovery; `Esc` →
  quit wizard.
- **W2 Discovery.** Immediate spinner ("Detecting settings for
  `user@domain`…", deadline 15 s) while the `DiscoverConfig` operation
  runs. Results render as a ranked list (see §3.3): one row per IMAP+SMTP
  candidate pair, labeled with its source (`Thunderbird autoconfig`,
  `DNS SRV (RFC 6186)`, `PACC`, `known provider: Gmail`, `manual`).
  Highest-ranked pair is preselected. Additional actions:
  `Enter` accept, `r` re-run discovery, `e` **manual override**.
  - *Manual override* is a W2 sub-form: editable IMAP server URL
    (default `imaps://` + guessed host from domain, e.g.
    `imaps://imap.<domain>:993`), SMTP server URL
    (`smtps://smtp.<domain>:465`), and username (default: full address).
    Its result is tagged `source = manual` and skips straight to W4.
  - Empty result: the screen explains discovery found nothing and offers
    the same manual-override form as the only way forward.
- **W3 Identity.** Display name (optional; default suggestion: the local
  part of the address) and a read-only recap of the chosen servers.
  `Esc` returns to W2.
- **W4 Credentials.** Fields:
  - **Username** (prefilled with the email address; some providers use a
    different login).
  - **Storage mode** toggle: `store password in config` vs
    `fetch via command`.
    - *Raw*: masked password field (keystrokes render as `*`).
    - *Command*: a command line producing the secret on stdout
      (e.g. `pass show mail/gmail`). Tmail validates it the same way the
      editor command is validated (`config::program_exists`, no shell
      metacharacters — Tmail never spawns a shell) and stores it as
      `password.cmd` verbatim; the command is never executed by the
      wizard itself — himalaya executes it during the connection test.
  - Gmail hint: when the account domain is `gmail.com`/`googlemail.com`
    (or discovery tagged the provider Google), the screen shows a
    one-line note that IMAP with a normal Google password requires an
    **app password** (Google account with 2FA).
  - v1 uses the same SASL PLAIN credentials for IMAP and SMTP
    (matching himalaya's own wizard's default path); a per-service
    override is future work.
- **W5 Testing.** "Testing IMAP connection…" while `TestAccount` runs
  (§3.4). Failure returns to W4 with the sanitized error and the fields
  intact; success carries the mailbox listing into W6.
- **W6 Aliases / confirm.** Shows the derived `mailbox.alias` map
  (§3.5), the account name, email, display name, servers, storage mode
  (raw shown as `****`), and the target file path. `Enter` saves.
- **W7 Saved.** Brief confirmation; `Enter`/`Esc` continues into the
  normal mailbox UI (first-run) or exits (manual).

Keyboard map: `↑/↓` move, `Tab`/`Shift+Tab` cycle fields, `Enter`
activate, `Esc` back/cancel, `Ctrl+C` quit app. The wizard binds no new
global keys; keymap defaults apply inside the wizard context.

### 3.3 Discovery integration

**Dependency.** `io-pim-discovery = "=0.7"` with the email-relevant
feature set: `autoconfig`, `pacc`, `rfc6186`, `stream` (std blocking
compose client), `rustls-ring` (default TLS). Calendar/contacts/JMAP/
OAuth-metadata features stay off.

**Wrapper.** A thin adapter isolates the volatile 0.x API:

```rust
// src/discovery/mod.rs
pub struct DiscoveredService {
    pub source: ConfigSource,        // Provider | Autoconfig | Pacc | Rfc6186 | Manual
    pub imap: ServerEndpoint,        // url, security (Tls | StartTls | None)
    pub smtp: ServerEndpoint,
    pub provider: Option<Provider>,  // Gmail | Outlook | … (display + presets)
}

#[async_trait::async_trait]
pub trait EmailConfigDiscoverer: Send + Sync {
    async fn discover(&self, email: &str) -> Vec<DiscoveredService>;
}
```

- `PimDiscoverer` calls
  `DiscoveryComposeClientStd::compose_all_within(Duration::from_secs(15))`
  on `tokio::task::spawn_blocking` (the client is blocking and spawns
  its own mechanism threads; the deadline bounds a hung endpoint). The
  `Vec<DiscoveryServiceConfig>` output is filtered to IMAP incoming and
  SMTP/submission outgoing endpoints — **POP results are discarded**
  (no himalaya backend), JMAP results are ignored in v1.
- **Pairing**: discovered configs are grouped into one candidate per
  provider/source; the first IMAP endpoint is paired with the first
  SMTP/submission endpoint from the same source, falling back to any
  SMTP endpoint. Single-sided candidates (IMAP without SMTP) still
  appear, flagged `smtp: not found` — the user can supply SMTP in the
  manual-override form.
- **Ranking**: known-provider rules first, then TLS endpoints over
  STARTTLS over plaintext, then mechanism priority (PACC ≈ autoconfig ≫
  RFC 6186 SRV). Ties keep the collector's order.
- **URL mapping**: TLS → `imaps://host:port` / `smtps://host:port`;
  STARTTLS → `imap://`/`smtp://` plus `imap.starttls = true` /
  `smtp.starttls = true`; plaintext likewise without `starttls`.
  Credentials always map to `sasl.plain.*` in v1 (the mechanism
  advertised by discovery is recorded for a future SCRAM/OAUTH2 path).
- The trait object is constructed in `main.rs` (`Arc<dyn
  EmailConfigDiscoverer>`); smoke tests and CI inject a fake via the
  existing dependency-injection point (`TMAIL_FAKE_DISCOVERY=1` selects
  a canned in-crate fake). No test path ever performs real DNS/HTTP.

**Privacy note** (documented in README alongside the feature): discovery
queries public infrastructure — the Thunderbird ISPDB, `autoconfig`
well-known URLs, DNS resolvers — and therefore reveals the *domain* (and
in some autoconfig query strings the full address) to those services.
Discovery only runs when the user submits the email screen.

### 3.4 Credential test (before anything is saved)

The reducer emits `Effect` kind `TestAccount`:

- The operation side (spawned like every backend operation) first
  serializes the **draft account** — servers, SASL credentials, alias
  table — into a fresh TOML document and writes it to
  `$TMPDIR/tmail-wizard-<uuid>.toml` with mode `0600` (the write uses
  `tempfile`'s guarded creation, already a dependency, then
  `set_permissions(0o600)` before any secret is placed in the file).
- It then runs `himalaya -c <temp> mailbox list -a <account> --json`
  through the same process-invocation plumbing the backend adapter uses
  (`src/backend/himalaya/process.rs`), with a 30 s timeout and
  cancellation wired to the wizard's cancel token.
- The listing JSON is parsed with the existing `MailboxesDto` shape and
  returned to the reducer as `BackendCompleted(TestAccountCompleted {
  mailboxes } | TestAccountFailed { error })`. The temp file is deleted
  in all outcomes (success, failure, cancellation); a leaked file from a
  hard kill is acceptable (0600, tempdir, OS cleanup).
- The failure path sanitizes himalaya's stderr through
  `crate::app::sanitize::sanitize` — credential values must never reach
  the UI or the logs (same rule as config-load issues).

The real config file is **not** touched until W6 confirms. Retrying
re-serializes with the edited values; nothing accumulates on disk.

### 3.5 Mailbox alias derivation

From the tested mailbox listing (names only):

1. **Gmail preset** when the provider is Gmail (discovery tag or
   `imap.gmail.com` host): map the fixed well-known folders —
   `INBOX → inbox`, `[Gmail]/Sent Mail → sent`, `[Gmail]/Drafts →
   drafts`, `[Gmail]/Trash → trash`, `[Gmail]/All Mail → archive` —
   keeping only entries whose target exists in the listing; always set
   `inbox = "INBOX"`.
2. **Generic heuristics** otherwise (case-insensitive, exact-name
   matching): `sent`/`sent items`/`sent messages` → `sent`;
   `drafts`/`draft` → `drafts`; `trash`/`deleted`/`deleted items`/
   `deleted messages` → `trash`; `archive`/`all mail` → `archive`;
   `INBOX` → `inbox`. First match wins; a role whose mailbox was already
   claimed is skipped. Unmatched roles are simply omitted — the parser
   already tolerates a partial alias table, and himalaya's special-use
   fallback covers the rest once io-imap grows `SPECIAL-USE` support.
3. The pure function `derive_aliases(mailbox_names, provider) ->
   Vec<(role, name)>` lives next to the discovery adapter and is fully
   unit-tested with canned listings (Gmail, Fastmail-style, bare dovecot).

### 3.6 Config file writing

New module `src/config/write.rs` — pure, dependency-injected, fully
unit-tested:

- **Format-preserving merge** with `toml_edit` (a new dependency;
  `toml` alone loses comments). Read the existing document (or an empty
  one), replace/insert `[accounts.<name>]` wholesale, re-serialize.
  Existing accounts, `[tmail]` tables, comments, and ordering survive.
- **Account block shape** matches the target example exactly: the
  `[accounts.<name>]` header plus dotted keys
  (`imap.server`, `imap.sasl.plain.username`,
  `imap.sasl.plain.password.raw|.cmd`, `smtp.*`, `mailbox.alias.*`) so
  the file reads like himalaya's own wizard output. Built by composing a
  TOML fragment string and parsing it with `toml_edit` (avoids fighting
  `toml_edit`'s implicit-table API for dotted keys), then inserted.
- **Account name**: the domain's first label (`gmail`, `fastmail`,
  `example`), lowercased, sanitized to `[a-z0-9-]`; on collision with an
  existing account the wizard asks: replace the existing block or pick
  `-2`, `-3`… (default: replace, since the user is re-running setup for
  that account).
- **`default = true`**: set on the new account only when no existing
  account has it; otherwise the pre-existing default stays authoritative
  (himalaya and `config::default_account` both resolve first-match).
- **Permissions**: a newly created file is created with mode `0600`
  (`OpenOptions::mode(0o600)` — never world-readable at any instant).
  An existing file keeps its mode; if it is group/world-readable **and**
  the wizard is about to write `password.raw`, W6 shows a warning line
  ("existing config is readable by others; run chmod 600") — Tmail does
  not silently chmod a user-owned file. With `password.cmd` no secret is
  stored and no warning is needed.
- **Post-write validation**: re-parse the merged file with the existing
  `config::parse_with_issues` and only report success if the new account
  resolves (guards against a `toml_edit` shape mistake).

### 3.7 Architecture fit

- **Reducer**: `AppState` gains `wizard: Option<WizardState>`
  (`src/app/wizard.rs`); new `Action::Wizard(WizardAction)` variants
  (`SubmitEmail`, `SelectService`, `OverrideServers`, `SubmitCredentials`,
  `Cancel`, `Retry`, `ConfirmSave`) and two `OperationKind` additions:
  `DiscoverConfig { email: String }` and
  `TestAccount { draft: Box<DraftAccountConfig> }`. All transitions stay
  I/O-free; secrets live only in `WizardState` and the boxed
  `DraftAccountConfig` payload. `tracing` never logs them (the sanitize
  rule applies to every wizard-originated message).
- **UI**: `src/ui/screens/wizard.rs` renders the current step as a
  full-screen panel (not a modal over the shell — the shell has no data
  to show during first run). Reuses the existing theme tokens, hint row,
  and focus machinery; a `Route::Wizard` variant guards against
  mailbox-key actions leaking into wizard steps.
  Utilize existing color schemes, so in future it should be possible to switch theme
  using existing theme switching mechanism 
- **Startup**: `main.rs` branches once — wizard route + no warmup
  effects when triggered; the normal loop, renderer, event stream, and
  `OperationManager` are unchanged. The discoverer is passed to the
  manager like the backend/opener are.

## 4. Implementation plan

Ordered, each step testable on its own.

- **W0 — Dependencies & scaffolding.** Add `io-pim-discovery = "=0.7"`
  (features: `autoconfig`, `pacc`, `rfc6186`, `stream`, `rustls-ring`)
  and `toml_edit`. Verify `cargo check --all-targets --all-features`
  stays green and note the binary-size delta.
- **W1 — Discovery adapter.** `src/discovery/mod.rs`: trait,
  `PimDiscoverer` (`spawn_blocking` + 15 s deadline), filtering (drop
  POP/JMAP), pairing, ranking, URL/security mapping, canned fake for
  tests. Unit tests with recorded mechanism outputs (committed fixtures,
  `fixtures/discovery/*.json`).
- **W2 — Alias derivation.** `derive_aliases` + unit tests (Gmail
  preset, heuristics, partial tables, duplicate-claim skipping).
- **W3 — Config writer.** `src/config/write.rs`: account block builder
  (raw + cmd variants), fresh-file write with 0600, format-preserving
  merge over a file containing `[tmail]` tables, comments, and other
  accounts, name collision handling, post-write re-validation. Tests
  pin the byte-exact expected output for the fresh case (the target
  shape from §1).
- **W4 — Wizard state & reducer.** `src/app/wizard.rs`: step machine,
  `WizardAction`s, `DiscoverConfig`/`TestAccount` operation kinds,
  suppression of warmup while wizard is active. Reducer tests cover the
  full success path, each failure path (discovery empty, test failure →
  retry, cancel at every step), and the no-secrets-in-logs invariant.
- **W5 — Wizard UI.** `src/ui/screens/wizard.rs` + `Route::Wizard`:
  screens W1–W7, masked password field, manual-override form, focus and
  hint rows, `--configure` flag parsing in `main.rs`. Snapshot tests
  follow the existing `tests/terminal_snapshots.rs` pattern.
- **W6 — Wiring & docs.** `main.rs` trigger logic (first-run + manual),
  fake-discoverer env hook, README section (including the privacy note
  and the Gmail app-password hint), `config.example.toml` untouched.
- **W7 — Smoke & contract tests.** Extend `tests/fake_himalaya.rs` with
  a `mailbox list` success/failure mode; pty smoke drives the whole
  wizard (fake discovery, fake himalaya, temp `HOME`) through save and
  asserts the written file and the `0600` mode.

Required checks after every step (per `AGENTS.md`): `cargo fmt --all`,
`cargo check --all-targets --all-features`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test --all-features`.

## 5. Testing strategy

- **Unit (no network, no subprocess):** discovery mapping/ranking,
  alias derivation, config writer (fresh, merge, collision, perms,
  cmd-vs-raw), wizard reducer paths, save-target resolution.
- **Contract:** `tests/fake_himalaya.rs` answers `mailbox list` so the
  `TestAccount` operation is exercised end-to-end against the real
  process plumbing; a failing fake covers the retry path.
- **PTY smoke:** full first-run wizard in a fake terminal with
  `TMAIL_FAKE_DISCOVERY=1` and a fake `himalaya` on `PATH`; asserts the
  saved file content, file mode, and that the app proceeds to the
  mailbox screen afterwards. CI runs it on macOS and Linux as today.
- **Manual QA checklist:** gmail.com with a real app password;
  a custom-domain server with only SRV records; discovery blackout
  (offline) → manual override; existing multi-account config merge.

## 6. Alternatives considered

- **Delegate to himalaya's built-in wizard** (bare `himalaya` on master
  runs discovery and prints TOML to stdout). Rejected for v1: its
  prompt-driven flow owns the whole terminal (exits the TUI), its
  account-name/SASL prompts differ from Tmail's UX, its output must
  still be merged into the shared file, and its behavior is
  version-dependent (2.0 prints to stdout; newer README says it writes
  to disk). The library path gives stable, in-process control.
- **Shell out to the `pim-discovery` CLI.** Rejected: a second binary
  users must install, an extra JSON contract to keep in sync, and no
  benefit over the in-process trait boundary (which the fake discoverer
  already provides for tests).
- **Plain terminal prompts before the TUI starts.** Rejected by product
  decision: the in-app wizard keeps the keyboard-first model, reuses
  theme/focus/rendering, and can return into the app without restart.
- **Store nothing (keyring/cmd only).** Rejected for v1 by product
  decision; `password.cmd` is offered as the hygienic choice and the
  raw path is explicit with 0600 enforcement.
- **JMAP/Gmail-REST accounts in the wizard.** Deferred: Tmail's feature
  set (aliases, trash/archive flows) is IMAP-shaped today; discovery
  already yields JMAP configs, so the adapter's filter is the only
  change needed later.
