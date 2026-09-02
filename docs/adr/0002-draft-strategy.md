# ADR 0002: Draft storage and replacement strategy

Date: 2026-09-02
Status: Accepted
Phase: 0 (updated after draft spike)

## Context

The composer needs continuous autosave (2 s debounce) without ever losing the
last known-good draft. The plan requires a spike to determine whether the
Himalaya CLI can update a remote draft in place before Post relies on it.

## Findings from the Phase 0 draft spike (himalaya 2.1.0)

Verified against a disposable Maildir backend:

1. **Create**: `message add -m Drafts --flag draft` accepts a raw RFC 5322
   message on stdin/file and returns `{"id":"...","sent":false}`. The `draft`
   flag is stored (`\Draft` in envelope flags).
2. **Update in place: NOT AVAILABLE.** There is no `message update` command.
   The shared CLI offers only `add`, `copy`, `move`, `delete`, `read`,
   `send`, `compose`, `reply`, `forward`.
3. **Delete is trash-first**: `message delete -m Drafts <id>` returns
   `{"action":"moved-to-trash"}` (the message lands in the trash alias under
   a NEW maildir id that is not returned), and only a second delete against
   the trash mailbox returns `{"action":"deleted"}` and actually removes it.
4. **Move changes ids** (maildir rename). Only the RFC `Message-ID` header
   survives relocation.
5. The Gmail-specific API (`gmail drafts …`) has proper draft update
   semantics but is provider-specific and forbidden to Post by design
   (plan §1/§3); Post cannot use it without breaking the shared-CLI rule.

## Decision

Post v1 implements the plan's **fallback strategy** (plan §14), refined by
findings 3–4:

1. **Crash-safe local draft journal** (Post-owned, e.g.
   `~/…/post/drafts/<local-id>.json` + `.eml` written via temp-file + atomic
   rename). Every revision is recorded before any remote call. This is draft
   state, not a mail cache; it exists so a crash mid-remote-save can never
   lose text the user typed.
2. **Remote saves are coalesced** (only the newest revision is ever pushed;
   an in-flight save of revision N is followed by another save if N+1
   exists).
3. **Replacement is add-then-delete**: push the new revision via
   `message add -m Drafts --flag draft`, confirm the new id from stdout, and
   only then delete the old remote draft.
4. **Old-draft deletion is best-effort and two-phase**:
   `message delete -m Drafts <old-id>` (→ moved to trash), then locate the
   moved message by `Message-ID` in the trash mailbox and delete again for
   permanent removal. Failure to complete phase 2 leaves a copy in trash —
   acceptable (never data loss), reconciled opportunistically later.
5. **Reconciliation on startup/save** lists the Drafts mailbox, matches by
   Post-owned `X-Post-Draft-Id` header (written into the draft MIME),
   and removes stale remote copies only after a newer copy is confirmed.
6. The draft's `Message-ID` header is **stable across revisions** of the
   same draft, which is what makes (4) and (5) possible.

## Consequences

- The autosave state machine (edit → dirty → debounce → saving → saved) can
  treat "remote save" as a single async operation per revision; the add+delete
  sequence is one backend operation returning the new remote id.
- Duplicate drafts can transiently exist during replacement; the product
  accepts this (plan §14 fallback step 6: never risk the last known-good draft
  for deduplication).
- `SentButCopyFailed`/ambiguous send handling (ADR 0001 finding 12) is
  orthogonal: sent drafts are resolved after send by deleting the draft
  (best-effort, same two-phase delete).
- The journal format must be versioned from day one (`version = 1`) so later
  migrations are possible.
