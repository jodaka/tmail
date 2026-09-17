"""Manual refresh (Ctrl+R) picks up mail that arrived after startup."""

from __future__ import annotations

import uuid

import imap_helpers


def test_manual_refresh_shows_new_mail(app):
    marker = f"E2EREFRESH-{uuid.uuid4().hex[:8]}"
    imap_helpers.append_message(marker)
    try:
        # Act: the message was appended after the startup page loaded.
        app.press("ctrl+r")
        app.expect("Refreshing…", timeout=15)

        # Assert: the refreshed page headlines the newest message.
        app.expect(marker, timeout=20)
    finally:
        imap_helpers.delete_by_subject(marker)
