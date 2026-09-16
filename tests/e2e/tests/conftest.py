from __future__ import annotations

import os
import re
import time
from collections.abc import Iterator
from pathlib import Path

import pexpect
import pytest


# tests/e2e/tests/conftest.py: find the repo root by its Cargo.toml so the
# default paths survive the harness moving under the tree.
def _repo_root() -> Path:
    for parent in Path(__file__).resolve().parents:
        if (parent / "Cargo.toml").is_file():
            return parent
    raise RuntimeError("repo root (Cargo.toml) not found above conftest.py")


REPO_ROOT = _repo_root()
E2E_DIR = REPO_ROOT / "tests" / "e2e"

# Host layout: tmail is built by the host cargo (run-tests.sh) and drives the
# Docker GreenMail stack through the config file below.
DEFAULT_TMAIL_CMD = str(REPO_ROOT / "target" / "debug" / "tmail")
DEFAULT_TMAIL_CONFIG = str(E2E_DIR / "tmail-e2e.toml")


class TuiApp:
    def __init__(self, child: pexpect.spawn):
        self.child = child

    def expect(self, text: str, timeout: float = 10.0) -> None:
        self.child.expect_exact(text, timeout=timeout)

    def expect_regex(self, pattern: str, timeout: float = 10.0) -> None:
        self.child.expect(re.compile(pattern), timeout=timeout)

    def send(self, text: str) -> None:
        self.child.send(text)

    def press(self, key: str) -> None:
        keys = {
            "enter": "\r",
            "esc": "\x1b",
            "tab": "\t",
            "up": "\x1b[A",
            "down": "\x1b[B",
            "right": "\x1b[C",
            "left": "\x1b[D",
            "ctrl+a": "\x01",
            "ctrl+r": "\x12",
        }
        self.child.send(keys.get(key.lower(), key))

    def assert_alive(self, settle_seconds: float = 1.0) -> None:
        time.sleep(settle_seconds)
        assert self.child.isalive(), (
            "TUI exited unexpectedly. Last terminal output:\n"
            + (self.child.before or "")
        )


@pytest.fixture
def app() -> Iterator[TuiApp]:
    command = os.environ.get("TMAIL_CMD", DEFAULT_TMAIL_CMD)
    # tmail reads TMAIL_CONFIG itself and forwards it to himalaya as `-c`;
    # the harness pins it to the bundled E2E config so tests never touch a
    # personal account file.
    os.environ.setdefault("TMAIL_CONFIG", DEFAULT_TMAIL_CONFIG)

    # A shell command is intentional here: TMAIL_CMD may include CLI arguments such as
    # `--config /path/to/config.toml`.
    child = pexpect.spawn(
        "/bin/bash",
        ["-lc", f"exec {command}"],
        env=os.environ.copy(),
        encoding="utf-8",
        codec_errors="replace",
        timeout=10,
        dimensions=(40, 120),
    )

    instance = TuiApp(child)
    try:
        yield instance
    finally:
        if child.isalive():
            # Generic TUIs cancel on `q`; tmail quits on Ctrl+C (its `quit`
            # binding). Sending both covers either binding, then a hard close.
            child.send("q")
            child.send("\x03")
            try:
                child.expect(pexpect.EOF, timeout=1)
            except pexpect.TIMEOUT:
                child.close(force=True)
