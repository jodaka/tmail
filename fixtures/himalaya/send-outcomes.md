# Phase 0 send-outcome characterization
#
# Real probes against himalaya 2.1.0 (`message send`) with a local SMTP sink
# (fixtures/himalaya/smtp_sink.py). Recorded 2026-09-02, macOS arm64.
#
# | Scenario                        | Exit | stdout (JSON)                                        | Tmail classification      |
# |---------------------------------|------|------------------------------------------------------|--------------------------|
# | sink accepts, normal send       | 0    | {"message":"Message successfully sent"}              | Sent                     |
# | send + --save missing mailbox   | 1    | {"error":"path .../NoSuchBox is not a directory"}    | FailedBeforeDelivery*    |
# | connection refused (dead port)  | 1    | {"error":"connect 127.0.0.1:3425"} sources[...61]    | FailedBeforeDelivery     |
# | sink closes after DATA payload  | 1    | {"error":"SMTP DATA failed: Reached unexpected EOF"} | Unknown                  |
#
# * the --save target is resolved BEFORE delivery; nothing is transmitted, so
#   this is a pre-delivery failure. Verified: sink captured no payload.
#
# The DATA-phase EOF case was verified as AMBIGUOUS: the sink logged
# `payload transmitted=True` while himalaya reported an error that looks
# definitive. Tmail must classify transport errors during/after DATA as
# `SendOutcome::Unknown` and warn that a retry may duplicate the message.
#
# Classification algorithm adopted by Tmail's backend adapter:
#   exit 0                  -> Sent
#   exit != 0, error text or
#   sources match pre-DATA
#   phase markers           -> FailedBeforeDelivery (detail = sanitized stderr)
#   otherwise               -> Unknown (conservative; includes killed child,
#                              EOF/reset/timeout during DATA, any unparseable
#                              error after the command started)
#
# `SentButCopyFailed` remains representable but is not currently producible
# through the shared CLI with maildir; on IMAP accounts a failed APPEND after
# SMTP delivery would surface as a generic error, which the conservative
# Unknown bucket also covers.
