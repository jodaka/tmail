"""Select-all + bulk mark-as-read, end to end.

Arrange (straight IMAP, like the seeder) → act (TUI keystrokes through a
real PTY) → assert (IMAP flags written by tmail's himalaya calls).
"""

from __future__ import annotations

import time

import pexpect

import imap_helpers


def _select_all(app, total: int) -> None:
    """Ctrl+A until the selection bar names the whole inbox.

    Startup races the page load (the sidebar counter arrives before the
    envelope page), so the first Ctrl+A may mark only the rows loaded so
    far. Select-all is idempotent on a partial page (insert-only), so
    retrying until the count matches is safe; it also proves the message
    list finished loading with the whole inbox on one page.
    """
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        app.press("ctrl+a")
        try:
            app.expect_regex(rf"{total} selected", timeout=2)
            return
        except pexpect.TIMEOUT:
            continue
    raise AssertionError(f"selection bar never showed {total} selected")


def test_select_all_and_mark_read(app):
    # Arrange: everything unread, so the final IMAP assertion has a full
    # set to prove. The count pins the expectation to the actual inbox size.
    imap_helpers.clear_flag(imap_helpers.uids("INBOX"), "\\Seen")
    total = len(imap_helpers.unseen_uids())
    assert total > 0, "inbox is empty; seed the mailbox first (./mail-only.sh)"

    # Act: select every message of the single page, then mark read.
    _select_all(app, total)
    app.press("i")
    # The status names the whole batch; it also proves `i` routed to the
    # bulk path, not the single-message one. The PTY stream splits status
    # words into styled spans, so ANSI bytes may sit between the tokens.
    app.expect_regex(rf"Marking.{{0,30}}{total} messages read", timeout=15)

    # Assert on the server: every message now carries \Seen. The change
    # rides one background himalaya call; tmail may still be draining its
    # startup page-warm operations, so the write can lag the status text.
    imap_helpers.wait_until(
        lambda: not imap_helpers.unseen_uids(),
        description="every message carries \\Seen",
    )
