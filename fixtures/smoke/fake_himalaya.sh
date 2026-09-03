#!/usr/bin/env bash
# Deterministic fake `himalaya` for the CI smoke (fixtures/smoke/ci_smoke.py).
#
# The smoke must run on both macOS and Linux runners without a real
# himalaya install, so this fake answers the two read-only commands the
# app needs to render its first screen. It emits the same JSON shapes the
# contract-test fake (tests/fake_himalaya.rs) uses. Read-only: it records
# nothing and touches nothing outside stdout.

set -u

SUB=""
OP=""
prev=""
for a in "$@"; do
  case "$prev" in
    message) OP="$a" ;;
  esac
  case "$a" in
    mailbox) SUB="mailbox" ;;
    envelope) SUB="envelope" ;;
    message) SUB="message" ;;
  esac
  prev="$a"
done

if [ "$SUB" = "mailbox" ]; then
  printf '%s' '{"mailboxes":[{"id":"/root/maildir/INBOX","name":"INBOX","total":null,"unread":null},{"id":"/root/maildir/Archive","name":"Archive","total":null,"unread":null},{"id":"Drafts","name":"Drafts","total":null,"unread":null},{"id":"Sent","name":"Sent"}]}'
  exit 0
fi

if [ "$SUB" = "message" ] && [ "$OP" = "add" ]; then
  # Draft save (plan §14): the backend id of the new copy.
  printf '%s' '{"id":"draft-1","sent":false}'
  exit 0
fi

if [ "$SUB" = "mailbox" ]; then
  printf '%s' '{"mailboxes":[{"id":"/root/maildir/INBOX","name":"INBOX","total":null,"unread":null},{"id":"/root/maildir/Archive","name":"Archive","total":null,"unread":null},{"id":"Sent","name":"Sent"}]}'
  exit 0
fi

if [ "$SUB" = "envelope" ]; then
  printf '%s' '{"envelopes":[{"id":"env-1","message-id":"1@post.local","in-reply-to":[],"flags":[{"raw":"\\Flagged","iana":"flagged"}],"subject":"Welcome","from":[{"name":"Ada","email":"ada@example.org"}],"to":[{"name":null,"email":"probe@post.local"}],"date":"2026-09-02T10:03:40+03:00","size":319,"has-attachment":false}]}'
  exit 0
fi

printf '%s' '{"error":"unsupported subcommand"}'
exit 1
