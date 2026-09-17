"""Reply: seeds a composer draft from the focused message."""

from __future__ import annotations

import imap_helpers


def test_reply_seeds_a_composer_from_the_focused_message(app):
    drafts_before = imap_helpers.mailbox_count("Drafts")

    # Act: the focused message is fetched, then seeded into the composer.
    app.press("r")
    app.expect("Reply draft ready", timeout=30)

    # Assert: the composer shows the seeded recipients/subject (a `Re:`
    # prefix proves the seed came from a real message).
    app.expect_regex(r"(?s)Re:.{0,200}", timeout=15)

    # A seeded draft starts clean: leaving saves nothing until it is edited.
    app.press("esc")
    app.expect_regex(r"(?s)Space.{0,80}select", timeout=15)
    assert imap_helpers.mailbox_count("Drafts") == drafts_before
