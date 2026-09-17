"""The shortcuts overlay opens, lists bindings, and closes."""

from __future__ import annotations


def test_help_overlay_opens_and_closes(app):
    app.press("?")
    app.expect("Shortcuts", timeout=15)
    app.expect("Reply", timeout=15)
    app.expect("Esc or ? closes", timeout=15)

    # A reopened overlay proves the Esc actually dismissed it: were it still
    # open, `?` would close it and no fresh title would be drawn.
    app.press("esc")
    app.press("?")
    app.expect("Shortcuts", timeout=15)
    app.press("esc")
    app.assert_alive(0.5)
