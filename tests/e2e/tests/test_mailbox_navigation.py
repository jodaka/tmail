"""Message-list navigation and the reader, end to end."""

from __future__ import annotations

import imap_helpers


def _expect_reader_headers(app) -> None:
    """The reader's header block: From, then To, then Date."""
    app.expect_regex(r"(?s)From.{0,200}To.{0,200}Date", timeout=15)


def test_reader_open_marks_read_and_navigation_moves_focus(app):
    ids = imap_helpers.uids("INBOX")
    assert ids, "inbox is empty; seed the mailbox first (./mail-only.sh)"

    # Arrange: everything unread, so each open has a full set to prove.
    imap_helpers.clear_flag(ids, "\\Seen")
    total = len(ids)

    subjects = imap_helpers.subjects_by_date_desc()
    assert len(set(subjects[:2])) == 2, (
        "need two messages with distinct subjects to test navigation"
    )
    top, second = subjects[0], subjects[1]

    # Act: open the focused (first) message.
    app.press("enter")
    app.expect(top, timeout=15)
    _expect_reader_headers(app)

    # Assert: opening marks exactly the opened message read on the server.
    imap_helpers.wait_until(
        lambda: len(imap_helpers.unseen_uids()) == total - 1,
        description=f"one of {total} messages read after opening the reader",
    )

    # Back to the list, move the selection down, open the second row.
    app.press("q")
    app.expect_regex(r"(?s)Space.{0,80}select", timeout=15)

    app.press("down")
    app.press("enter")
    app.expect(second, timeout=15)
    _expect_reader_headers(app)

    imap_helpers.wait_until(
        lambda: len(imap_helpers.unseen_uids()) == total - 2,
        description="the second opened message is read too",
    )
