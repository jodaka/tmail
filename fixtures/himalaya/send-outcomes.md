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

# Ticket kws6 addendum (2026-10-07): `message send` is only safe for
# Bcc-less mail — himalaya 2.1.0 (io-smtp 0.3) transmits the DATA payload
# verbatim, so a `Bcc:` header would be disclosed to every To/Cc
# recipient. Sends that carry Bcc recipients now go through
# `smtp send --mail-from <account> --rcpt-to <to...> --rcpt-to <cc...>
# --rcpt-to <bcc...> --json`: the envelope is explicit (2.1.0 and 2.2.1
# expose the identical CLI, and 2.2.1 pins `keep_bcc: true` for this
# command), while tmail's serializer keeps the Bcc header off the DATA.
# Probed with smtp_sink.py against himalaya 2.2.1: identical
# `{"message":"Message successfully sent"}` JSON, the RCPT envelope
# carries the blind addresses, and the captured DATA holds no Bcc header.
# The outcome taxonomy below applies to this path unchanged.
#
# Ticket frmm addendum (2026-10-07): the 30 s `CALL_TIMEOUT` kill
# (ticket 183r) never produced an exit status, so it must not surface as
# a structural failure. The backend maps a send timeout into
# `SendOutcome::Unknown` (delivery ambiguous) in `send_message` — the
# "timeout during DATA → Unknown" line of the algorithm below now also
# covers the budget kill itself, whether it fires mid-DATA or after a
# silent restart.
