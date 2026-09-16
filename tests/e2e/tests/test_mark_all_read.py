"""Select-all + bulk mark-as-read, end to end.

Arrange (straight IMAP, like the seeder) → act (TUI keystrokes through a
real PTY) → assert (IMAP flags written by tmail's himalaya calls).
"""

from __future__ import annotations

import imaplib
import time

import pexpect

IMAP_HOST = "127.0.0.1"
IMAP_PORT = 3143
USERNAME = "tmail"
PASSWORD = "tmailtmail"

SETTLE_TIMEOUT = 60.0


def _imap() -> imaplib.IMAP4:
    return imaplib.IMAP4(IMAP_HOST, IMAP_PORT, timeout=5)


def _inbox_ids() -> list[bytes]:
    with _imap() as imap:
        imap.login(USERNAME, PASSWORD)
        imap.select("INBOX")
        _, data = imap.search(None, "ALL")
        return data[0].split()


def _unseen_ids() -> list[bytes]:
    with _imap() as imap:
        imap.login(USERNAME, PASSWORD)
        imap.select("INBOX")
        _, data = imap.search(None, "UNSEEN")
        return data[0].split()


def _flag_by_sequence(ids: list[bytes], remove: bool, flag: str) -> None:
    if not ids:
        return
    # GreenMail accepts the standard shape: the sign rides the op token
    # and the flag list follows as its own argument.
    operation = "-FLAGS.SILENT" if remove else "+FLAGS.SILENT"
    sequence = b",".join(ids).decode("ascii")
    with _imap() as imap:
        imap.login(USERNAME, PASSWORD)
        imap.select("INBOX")
        status, _ = imap.store(sequence, operation, "(" + flag + ")")
        if status != "OK":
            raise RuntimeError(f"could not apply {flag} to {len(ids)} messages")


def _wait_until(predicate, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.5)
    raise AssertionError(f"condition not met within {timeout}s: {predicate}")


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
    _flag_by_sequence(_inbox_ids(), remove=True, flag="\\Seen")
    total = len(_unseen_ids())
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
    _wait_until(lambda: not _unseen_ids(), SETTLE_TIMEOUT)
    assert len(_unseen_ids()) == 0
