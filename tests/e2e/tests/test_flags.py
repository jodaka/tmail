"""Message flags (star, read/unread) driven from the list, asserted via IMAP."""

from __future__ import annotations

import time

import imap_helpers


def test_star_toggles_the_flagged_flag(app):
    ids = imap_helpers.uids("INBOX")
    assert ids, "inbox is empty; seed the mailbox first (./mail-only.sh)"

    imap_helpers.clear_flag(ids, "\\Flagged")
    assert not imap_helpers.flagged_uids(), "arrange: no message is starred"

    app.press("s")
    imap_helpers.wait_until(
        lambda: len(imap_helpers.flagged_uids()) == 1,
        description="one message starred",
    )

    # The server write lands before the app applies the confirmed toggle to
    # its row; let that result arrive before flipping the flag back.
    time.sleep(1.0)
    app.press("s")
    imap_helpers.wait_until(
        lambda: not imap_helpers.flagged_uids(),
        description="the star removed again",
    )


def test_mark_unread_and_read_updates_the_server(app):
    ids = imap_helpers.uids("INBOX")
    assert ids, "inbox is empty; seed the mailbox first (./mail-only.sh)"

    # Arrange: everything read, so `u` has a visible effect to prove.
    imap_helpers.set_flag(ids, "\\Seen", add=True)
    assert not imap_helpers.unseen_uids()

    app.press("u")
    imap_helpers.wait_until(
        lambda: len(imap_helpers.unseen_uids()) == 1,
        description="the focused message marked unread",
    )

    app.press("i")
    imap_helpers.wait_until(
        lambda: not imap_helpers.unseen_uids(),
        description="the focused message marked read again",
    )
