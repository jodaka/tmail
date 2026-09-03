# Phase 12.7 — Himalaya CLI invocation latency

Decision gate from POST_IMPLEMENTATION_PLAN.md §22: "CLI invocation opens a
new connection and feels slow → Measure after vertical slices; consider
supported session reuse/local store before Pimalaya." The plan forbids
starting a Pimalaya backend in v1 unless measured behavior makes the product
unusable (§22, Phase 12.7).

## Method

Ten sequential invocations per command (fresh process each time, as Post
always does: argv-only, no shell, no session reuse), against a disposable
local Maildir with 10 seeded messages. Wall-clock around the child process,
including spawn, config parse, and JSON output.

Environment: macOS 15 (aarch64), himalaya 2.1.0 (+maildir), 2026-09-03.

## Results (n=10 each)

| Command | median | min | max |
| --- | ---: | ---: | ---: |
| `mailbox list --json` | 5 ms | 5 ms | 6 ms |
| `envelope list` (page 1) | 5 ms | 5 ms | 6 ms |
| `envelope list` (page 2, cold) | 5 ms | 5 ms | 5 ms |
| `message read` | 5 ms | 5 ms | 7 ms |
| `flag add seen` | 5 ms | 5 ms | 5 ms |
| `flag remove seen` | 5 ms | 5 ms | 6 ms |

Raw medians cluster at 5–6 ms; no invocation exceeded 7 ms.

## Conclusion

- On the backend Post targets for automated testing (local Maildir), one
  full CLI invocation costs ~5 ms — two orders of magnitude below any
  perceptible threshold for foreground UI operations. The per-invocation
  process model (ADR 0001 decision 2) is confirmed cheap; **no Pimalaya
  backend and no session reuse is justified by this evidence.**
- Remote accounts (IMAP/JMAP) add one network round trip per invocation by
  design of the CLI; that cost is inherent to the remote host, not to the
  process model. Post already treats every backend call as an async,
  cancellable operation with progress feedback (plan §11), which is the
  mitigation for slow hosts. Re-measuring on a real remote account is only
  warranted if users report sluggishness; architecture rewrites require
  evidence and explicit user approval (plan §22).
