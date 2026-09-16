from __future__ import annotations

import os


def test_tui_starts(app):
    """Black-box smoke test: the TUI launches successfully inside a real PTY."""
    app.assert_alive()

    expected = os.environ.get("TMAIL_EXPECT_TEXT", "").strip()
    if expected:
        app.expect(expected)
