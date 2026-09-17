"""Search the mailbox and leave the results again."""

from __future__ import annotations

import time
import uuid

import imap_helpers


def test_search_finds_message_by_body_and_escape_restores_list(app):
    marker = f"e2esearch{uuid.uuid4().hex[:8]}"
    subject = f"E2E search fixture {marker}"
    # The query word lives in the body, so the result row's subject (not the
    # header echo of the query) proves the backend matched the message.
    imap_helpers.append_message(subject, body=f"fixture body {marker}")
    try:
        app.press("/")
        time.sleep(0.5)
        app.send(marker)
        app.press("enter")

        app.expect_regex(r"SEARCH", timeout=20)
        app.expect(subject, timeout=20)

        app.press("esc")
        app.expect_regex(r"(?s)Space.{0,80}select", timeout=15)
    finally:
        imap_helpers.delete_by_subject(subject)
