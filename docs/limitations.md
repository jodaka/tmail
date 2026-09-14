# Known limitations

**v1 scope** (by design, per `TMAIL_IMPLEMENTATION_PLAN.md` §23): no
conversation threads, no multiple-account switching, no label management,
no settings/help UI, and no offline sync.

**Search:**

- Non-ASCII search text (e.g. Cyrillic) currently fails with
  `IMAP SEARCH failed: BAD` — the IMAP server rejects it via himalaya
  2.1.0. Folder names and message content render fine; only searching
  *in* non-ASCII text is affected.
- On local Maildir accounts (himalaya `maildir` backend), text search
  matches nothing: himalaya delegates text filters to the server, and a
  Maildir has none. Flag filters do work locally (`flag flagged`,
  `not flag seen`). Listing, reading, flags, and drafts all work on
  Maildir (covered by `tests/maildir_integration.rs`).

**Mouse:** with capture enabled, plain click-drag belongs to Tmail; hold
`Shift` or press `m` to select text ([details](features.md#mouse)).
