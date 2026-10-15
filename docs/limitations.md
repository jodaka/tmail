# Known limitations

**v1 scope** (by design, per `TMAIL_IMPLEMENTATION_PLAN.md` §23): no conversation threads, no label management, no settings/help UI, and no offline sync. 

**Search:**

- On local Maildir accounts (himalaya `maildir` backend), text search
  matches nothing: himalaya delegates text filters to the server, and a
  Maildir has none. Flag filters do work locally (`flag flagged`,
  `not flag seen`). Listing, reading, flags, and drafts all work on
  Maildir (covered by `tests/maildir_integration.rs`).
