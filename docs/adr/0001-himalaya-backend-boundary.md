# ADR 0001: Himalaya CLI as the exclusive mail backend

Date: 2026-09-02
Status: Accepted
Phase: 0

## Context

Post delegates accounts, credentials, protocols, mailbox semantics, search, and
delivery to the Himalaya CLI (v2.1.0 verified). Post must not contain
provider-specific protocol code, and Himalaya JSON types must not escape
`backend/himalaya/`.

## Findings from the Phase 0 probe (himalaya 2.1.0, macOS arm64)

All commands verified via `tokio::process::Command` with argv arrays (no
shell), config passed with `-c <path>`, account with `-a <name>`:

| Post operation  | Himalaya invocation (JSON output via `--json`)             |
|-----------------|------------------------------------------------------------|
| list mailboxes  | `mailbox list` → `{"mailboxes":[{id,name,total,unread}]}`  |
| list envelopes  | `envelope list -m <mbox> -p <N> -s <size>` → `{"envelopes":[...]}` |
| search          | `envelope search -m <mbox> [QUERY]` (Himalaya's own DSL, passed through unchanged) |
| read message    | `message read -m <mbox> <id>` (mail_parser serde dump)     |
| set flags       | `flag {add,remove,set} -m <mbox> --flag <f> <ids...>`      |
| archive/trash   | `message move --to <Archive>` / `message delete -m <mbox>` |
| drafts          | `message add -m Drafts --flag draft` (create only)         |
| send            | `message send [--save <mbox>]` (message via argv file or stdin) |
| attachments     | `attachment list <id>` / `attachment download <id> <part>` |

Verified behaviors that constrain the adapter:

1. **JSON output is valid but has no trailing newline**; errors with `--json`
   are also JSON objects (`{"error":..., "sources":[...]}`) on stdout with
   exit code 1. The adapter must treat exit status as authoritative and parse
   stdout as JSON in both cases.
2. **Envelope JSON**: `{id, message-id, in-reply-to[], flags[{raw,iana}],
   subject, from[{name,email}], to[], date (ISO-8601 with offset), size,
   has-attachment}`. There is **no snippet field**; Post's snippet is not
   available without a full `message read`. `total`/`unread` are `null` on the
   maildir backend; the UI must tolerate unknown totals (plan §16).
3. **Ordering**: `envelope list` sorts by date descending; ties are unordered.
   Fixtures and tests must not depend on tie order.
4. **Maildir ids are file names** and **change when a message is moved**
   (move/trash rewrites the file). Only the RFC `Message-ID` header is stable
   across moves. Post must therefore re-resolve selections by `Message-ID`
   after any move/delete, and cannot cache maildir ids across operations that
   relocate messages.
5. **`message delete` is trash-first**: it moves to the trash alias
   (`{"action":"moved-to-trash"}`), and reports `{"action":"deleted"}` when
   the message is already in trash. The new id after a trash move is **not
   returned**; it must be re-discovered by `Message-ID` in the trash mailbox.
6. **Flag commands echo the affected flags**, not the resulting state
   (`flag remove --flag seen` → `{"flags":["seen"]}`). Authoritative flag
   state must come from `envelope list`/`message read`. *Correction
   (Phase 4 probe)*: without `--json`, `flag {add,remove}` and
   `message {move,delete}` print **human text** on stdout
   (`Successfully added flags: flagged`) with exit 0 — Post must pass
   `--json` on every mutation so the adapter's JSON validation holds.
7. **All shared commands accept `-m <mailbox>`** and fall back to the inbox
   alias when omitted. Post must always pass the mailbox explicitly.
8. **Pagination is 1-based** (`-p`, `-s`); page size defaults to
   `envelope.list.page-size` config or 25. Post maps its 0-based
   `PageRequest{offset, limit}` to `p = offset/limit + 1`, `s = limit`.
9. **`has-attachment` in envelopes is opt-in** (`--has-attachment`) and costs
   a per-envelope lookup; Post will not use it for list rows and will derive
   the flag from `message read` when the reader is open.
10. **`message read --json`** returns the raw serde representation of
    `mail_parser::Message` (`parts`, `text_body: [part_idx]`, `html_body:
    [part_idx]`, header value tagged enums). It is verbose but complete and
    stable enough to map into Post domain types inside `backend/himalaya/`.
11. **Reply/forward templates exist and are structured**:
    `message reply -m <mbox> <id>` emits full RFC 5322 MIME on stdout with
    `In-Reply-To`, `References`, `Re:` subject, and quoted body prefilled
    (fixtures/himalaya/reply-template.eml). Post will use these as the base
    of composer drafts instead of hand-rolling reply headers (plan §14).
12. **Send outcomes**: see fixtures/himalaya/send-outcomes.md. Exit 0 =
    `{"message":"Message successfully sent"}`. Failures are JSON errors with
    exit 1; the ambiguous post-DATA failure case was reproduced
    (payload transmitted, himalaya reports EOF) and must map to
    `SendOutcome::Unknown`.
13. **Config coexistence**: a `[post]` section in the same TOML file as
    Himalaya accounts parses and runs cleanly (himalaya ignores unknown
    tables at the root). One canonical user config file works.
14. **`json-schema <DIR>`** generates 70 JSON Schemas for command outputs
    (fixtures/himalaya/schemas/), usable for contract tests.

## Decision

1. Post v1 shells out to the installed `himalaya` binary exclusively, through
   a single `MailBackend` trait implemented by `HimalayaCliBackend`. No
   protocol code, no Pimalaya library dependency in v1 (revisit only with
   measured evidence and user approval, plan §22).
2. All child processes: `tokio::process::Command` with argv arrays, stdin
   piping via a dedicated task, `kill_on_drop(true)` plus explicit SIGKILL on
   cancellation (proven in `src/bin/probe.rs`, ~150 ms cancel latency).
3. Himalaya DTOs live only in `backend/himalaya/dto.rs`; mapping to domain
   types happens in `backend/himalaya/map.rs` with fixture-backed tests.
4. Errors are classified into typed outcomes with sanitized details
   (`thiserror` types at the backend, `anyhow` context at the app layer).
5. Post writes its own `[post]` config section into the same TOML file
   himalaya reads (verified tolerated), keeping one canonical user file.

## Consequences

- Post inherits Himalaya's supported backends (IMAP, JMAP, Gmail, Maildir…)
  without implementing any of them.
- A fake `himalaya` executable + argv assertions become the core contract
  test strategy (Phase 2+).
- Version drift risk is mitigated by schema/fixture contract tests and a
  startup `himalaya --version` check.
- Maildir id instability (finding 4/5) forces Message-ID-based selection
  stability everywhere (plan §11 already requires this).
