# Mail caching in Post (ticket srn2)

Problem: Post opens with an empty list and a "Loading messages" spinner,
because every view is fetched from the Himalaya CLI on demand and Himalaya
v2 ships no message cache. On a local Maildir the fetch costs ~5 ms
(docs/phase-12-latency.md) and nobody notices; on IMAP each invocation
opens a fresh TCP+TLS+SASL session, so a cold start visibly waits on the
network. This document records what is actually supported (verified against
himalaya 2.1.0, September 2026), and what Post can do about it.

## 1. What Himalaya supports (verified)

### 1.1 No built-in message cache

Himalaya v2 is stateless by design ("CLI, not TUI"). There is **no**
message-cache configuration: no `[cache]` table, no envelope persistence,
no `cache.*` keys in the shipped `config.sample.toml`, and no cache-related
flags in `himalaya --help`. Every `envelope list` re-reads the backend.

### 1.2 Session reuse via sirup (official, config-only)

The official answer to per-invocation cost is [sirup](https://github.com/pimalaya/sirup):
a daemon that connects and authenticates **once**, then serves the live
IMAP/SMTP/ManageSieve session on a Unix socket (`PREAUTH` greeting, so no
credentials travel). Himalaya points its server at the socket:

```toml
[accounts.personal]
imap.server = "unix:///Users/me/.local/share/sirup/personal-imap.sock"
```

Effects and trade-offs:

- Removes the TCP+TLS+SASL handshake from every Post invocation — the
  dominant IMAP latency (phase-12-latency.md §Conclusion).
- Zero Post code: pure account config. Post needs nothing.
- The user must run and supervise a daemon (systemd/launchd unit), and the
  socket carries an **already-authenticated session** — any local process
  that can read the socket speaks IMAP as the user. Socket permissions and
  a trusted local machine are prerequisites.
- It accelerates the transport, it does **not** make a cold start
  instant: the first `envelope list` still does a full IMAP round trip.

### 1.3 Local mail stores (no network at all)

The `maildir`, `m2dir`, `pimdir`, and `notmuch` backends read local files.
They are inherently "cached" (and our fastest path, §latency). Keeping an
IMAP account mirrored locally is a real but heavyweight option — e.g.
running a sync tool (himalaya's sibling project neverest) on a schedule
plus a second Post account block. Out of scope for v1; noted for
completeness.

## 2. What Post can do (Post-owned cache)

The ticket's goal — "instantly show cached messages and update in the
background" — is achievable entirely inside Post, independent of the
backend:

**Summary cache.** Persist the last loaded page of envelope summaries per
mailbox (the `Page<MessageSummary>` the reducer already holds) into Post's
data directory, and load it at startup *before* the first backend request:

1. On every successful `LoadPage`/`Search` application, the reducer (or the
   completion path) writes the page to `~/…/post/cache/<account>/<mailbox>/
   <offset>-<limit>.json` (atomic write, sanitized — summaries contain no
   credentials; flag changes update the cached copy already in memory).
2. On startup/mailbox switch, the page loader serves the cached page
   immediately (marking rows visibly stale is optional) and then issues the
   normal background load; existing operation-superseding machinery applies
   the fresh page when it lands, and stale-result rejection is already
   proven (Phase 9/§11).
3. Cache invalidation is conservative: overwritten by every successful
   load, never trusted after a failed one, bounded to a few pages per
   mailbox, and silently ignored when unparsable.

This makes warm starts instant for maildir *and* IMAP, and pairs with
§1.2 for the first IMAP touch of a session. It is a small, additive change
to the existing page-loading path — no architecture rewrite, consistent
with the §22 decision gate (measure first, extend only on evidence).

## 3. Recommendation

1. **Adopt the Post-owned summary cache (§2)** for instant warm starts —
   it is backend-agnostic and small.
2. **Document sirup in the README** as the user-side fix for IMAP
   invocation latency; Post needs no code for it.
3. Skip local mail mirroring (§1.3) until a user asks for offline work —
   that is a different product decision, not a cache.

## 4. Follow-ups

- [x] Implement the summary cache (§2) — shipped with ticket haeb:
      `src/app/page_cache.rs` covers pages, the mailbox listing, and
      viewed messages, bounded by `[post.cache]` limits (LRU eviction).
- [ ] Add sirup configuration guidance to the README troubleshooting
      section if IMAP latency reports come in.
- [ ] Re-measure with a real IMAP account before and after (plan §22
      evidence gate).
