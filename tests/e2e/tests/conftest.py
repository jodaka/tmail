from __future__ import annotations

import os
import re
import shutil
import time
from collections.abc import Iterator
from pathlib import Path

import pexpect
import pytest

import imap_helpers


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

    def wait_ready(self, timeout: float = 20.0) -> None:
        """Block until the mailbox listing and its first page are on screen.

        The listing status arrives only after the server answered, and the
        range label (`1–12`) only once rows rendered.
        """
        try:
            self.expect("Mailboxes loaded", timeout=timeout)
            self.expect_regex(r"1–\d+", timeout=timeout)
        except pexpect.TIMEOUT as exc:
            raise AssertionError(
                "tmail did not finish its first load; is GreenMail running "
                "and seeded? (cd tests/e2e && ./mail-only.sh)"
            ) from exc


def _wait_for_warm_cache(
    data_dir: Path, expected: int, settle: float = 2.0, timeout: float = 90.0
) -> None:
    """Give the page-cache preview burst time to finish (best effort).

    The cold start fetches every visible row's preview through the shared
    backend permit pool; an action sent during the burst queues behind it.
    One cache file appears per fetched message, so waiting for nearly
    `expected` of them marks the burst's end. Stragglers are left alone —
    they hold one permit at most, and a hard wait has proven flakier than
    the queueing it avoids.
    """
    deadline = time.monotonic() + timeout
    stable_since = time.monotonic()
    count = 0
    while time.monotonic() < deadline:
        count = sum(1 for _ in (data_dir / "cache").rglob("messages/**/*.json"))
        if count >= expected - 1:
            if time.monotonic() - stable_since >= settle:
                return
        else:
            stable_since = time.monotonic()
        time.sleep(0.25)


def _spawn(command: str, env: dict[str, str]) -> pexpect.spawn:
    # A shell command is intentional here: TMAIL_CMD may include CLI
    # arguments such as `--config /path/to/config.toml`.
    return pexpect.spawn(
        "/bin/bash",
        ["-lc", f"exec {command}"],
        env=env,
        encoding="utf-8",
        codec_errors="replace",
        timeout=10,
        dimensions=(40, 120),
    )


def _close(child: pexpect.spawn) -> None:
    if child.isalive():
        # Generic TUIs cancel on `q`; tmail quits on Ctrl+C (its `quit`
        # binding). Sending both covers either binding, then a hard close.
        child.send("q")
        child.send("\x03")
        try:
            child.expect(pexpect.EOF, timeout=1)
        except pexpect.TIMEOUT:
            child.close(force=True)


@pytest.fixture(scope="session", autouse=True)
def semantic_mailboxes() -> None:
    """Create Trash/Archive/Drafts/Sent once, before any app starts.

    GreenMail exposes INBOX only; the E2E config aliases the four semantic
    mailboxes to these names (tmail-e2e.toml), so the app can resolve
    trash/archive/draft targets.
    """
    imap_helpers.ensure_mailboxes()


@pytest.fixture(scope="session")
def warmed_cache(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """A page cache warmed once, before the tests run.

    Tests must never read or write the developer's real draft journal or
    page cache. Warming the cache first means each test's app serves its
    previews from disk instead of spawning a burst of backend fetches
    that would make the test's own actions queue behind them.
    """
    data_dir = tmp_path_factory.mktemp("tmail-warm")
    command = os.environ.get("TMAIL_CMD", DEFAULT_TMAIL_CMD)
    os.environ.setdefault("TMAIL_CONFIG", DEFAULT_TMAIL_CONFIG)
    env = os.environ.copy()
    env["TMAIL_DATA_DIR"] = str(data_dir)
    child = _spawn(command, env)
    instance = TuiApp(child)
    try:
        instance.wait_ready()
        _wait_for_warm_cache(data_dir, imap_helpers.inbox_count())
    finally:
        _close(child)
    return data_dir / "cache"


@pytest.fixture
def app(
    tmp_path_factory: pytest.TempPathFactory, warmed_cache: Path
) -> Iterator[TuiApp]:
    command = os.environ.get("TMAIL_CMD", DEFAULT_TMAIL_CMD)
    # tmail reads TMAIL_CONFIG itself and forwards it to himalaya as `-c`;
    # the harness pins it to the bundled E2E config so tests never touch a
    # personal account file.
    os.environ.setdefault("TMAIL_CONFIG", DEFAULT_TMAIL_CONFIG)
    # Each test gets its own data dir (no draft-journal cross-talk), primed
    # with the warmed page cache (no preview-fetch burst at startup).
    data_dir = tmp_path_factory.mktemp("tmail-data")
    if warmed_cache.is_dir():
        shutil.copytree(warmed_cache, data_dir / "cache")
    os.environ["TMAIL_DATA_DIR"] = str(data_dir)

    child = _spawn(command, os.environ.copy())

    instance = TuiApp(child)
    try:
        instance.wait_ready()
        yield instance
    finally:
        _close(child)
