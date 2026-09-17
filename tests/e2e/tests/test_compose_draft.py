"""Compose: typing into the first field and leaving saves a draft."""

from __future__ import annotations

import time
import uuid

import imap_helpers


def test_compose_and_leave_saves_a_draft(app):
    recipient = f"e2e-compose-{uuid.uuid4().hex[:8]}@example.test"
    drafts_before = imap_helpers.mailbox_count("Drafts")

    app.press("c")
    # The composer is on screen once its attach row is drawn.
    app.expect("[ + attach ]", timeout=15)
    # The To field holds focus when the composer opens.
    app.send(recipient)
    time.sleep(1.0)
    # Esc saves & leaves (never a silent discard).
    app.press("esc")

    imap_helpers.wait_until(
        lambda: imap_helpers.mailbox_count("Drafts") == drafts_before + 1,
        description=f"drafts grows {drafts_before} -> {drafts_before + 1}",
    )
    imap_helpers.wait_until(
        lambda: any(recipient in to for to in imap_helpers.draft_recipients()),
        description="the saved draft carries the typed recipient",
    )
    imap_helpers.delete_by_header("To", recipient, mailbox="Drafts")
