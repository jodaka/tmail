"""Trash and archive move the focused message to its alias mailbox."""

from __future__ import annotations

import imap_helpers


def test_trash_moves_the_focused_message(app):
    inbox_before = imap_helpers.mailbox_count("INBOX")
    trash_before = imap_helpers.mailbox_count("Trash")
    assert inbox_before > 0, "inbox is empty; seed the mailbox first (./mail-only.sh)"

    app.press("d")
    app.expect("Moving to trash…", timeout=15)

    imap_helpers.wait_until(
        lambda: imap_helpers.mailbox_count("INBOX") == inbox_before - 1,
        description=f"inbox shrinks {inbox_before} -> {inbox_before - 1}",
    )
    imap_helpers.wait_until(
        lambda: imap_helpers.mailbox_count("Trash") == trash_before + 1,
        description=f"trash grows {trash_before} -> {trash_before + 1}",
    )
    app.expect("Message moved", timeout=15)


def test_archive_moves_the_focused_message(app):
    inbox_before = imap_helpers.mailbox_count("INBOX")
    archive_before = imap_helpers.mailbox_count("Archive")
    assert inbox_before > 0, "inbox is empty; seed the mailbox first (./mail-only.sh)"

    app.press("e")
    app.expect("Archiving…", timeout=15)

    imap_helpers.wait_until(
        lambda: imap_helpers.mailbox_count("INBOX") == inbox_before - 1,
        description=f"inbox shrinks {inbox_before} -> {inbox_before - 1}",
    )
    imap_helpers.wait_until(
        lambda: imap_helpers.mailbox_count("Archive") == archive_before + 1,
        description=f"archive grows {archive_before} -> {archive_before + 1}",
    )
    app.expect("Message moved", timeout=15)
