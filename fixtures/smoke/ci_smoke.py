#!/usr/bin/env python3
"""CI smoke (Phase 12.6): terminal lifecycle + command selection per platform.

Runs the real Tmail binary under a pty against a committed fake `himalaya`
(fake_himalaya.sh) — no network, no real mail, fully deterministic:

1. happy path: startup renders the fake inbox (proving the editor command
   resolved, since the config uses `editor = "$EDITOR"`), then Esc quits
   cleanly: exit code 0, leave-alternate-screen restore in the output, and
   no panic text (terminal lifecycle, plan §19 Phase 12 acceptance);
2. editor validation: the same `editor = "$EDITOR"` config with EDITOR
   unset must fail startup with an "editor" validation issue;
3. opener environment: the platform opener (`open` on macOS, `xdg-open`
   on Linux) exists on the runner, matching src/backend/opener.rs's
   compile-time selection (unit-tested per platform too).

Usage: python3 fixtures/smoke/ci_smoke.py --bin target/debug/tmail

Note: the app redraws on every 250 ms tick, so pty reads use fixed time
windows, never "read until quiet".
"""

import argparse
import fcntl
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import termios
import tempfile
import time

ROWS, COLS = 40, 152
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def strip_ansi(text):
    text = re.sub(r"\x1b\[[0-9;?]*[a-zA-Z]", "", text)
    text = re.sub(r"\x1b[()][0-9A-B]", "", text)
    return text


def read_for(fd, seconds):
    out = b""
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
    return out.decode("utf-8", "replace")


def wait_for(fd, needle, timeout=10.0):
    end = time.time() + timeout
    acc = ""
    while time.time() < end:
        acc += read_for(fd, 0.3)
        if needle in strip_ansi(acc):
            return acc
    raise AssertionError(
        f"timed out waiting for {needle!r}; got:\n{strip_ansi(acc)[-3000:]}"
    )


def spawn(argv, env_overrides):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        for key, value in env_overrides.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        os.execvp(argv[0], argv)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    return pid, fd


def wait_exit(pid, fd, timeout=10.0):
    """Read until the child exits; return (status, all_output)."""
    end = time.time() + timeout
    out = ""
    while time.time() < end:
        out += read_for(fd, 0.3)
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            out += read_for(fd, 0.3)
            return status, out
    raise AssertionError(
        f"process did not exit within {timeout}s; got:\n{strip_ansi(out)[-3000:]}"
    )


def write_config(path):
    with open(path, "w") as f:
        f.write(
            "[accounts.probe]\n"
            'email = "probe@tmail.local"\n'
            "\n"
            "[tmail]\n"
            'account = "probe"\n'
            "\n"
            "[tmail.composer]\n"
            'editor = "$EDITOR"\n'
        )


def step_happy(binpath, config, fake_dir):
    print(
        "== happy path: render, editor via $EDITOR, clean quit + restore ==", flush=True
    )
    pid, fd = spawn(
        [binpath, config],
        {"EDITOR": os.path.join(fake_dir, "true-like"), "PATH": os.environ["PATH"]},
    )
    try:
        wait_for(fd, "Welcome")
        print("   inbox rendered through the fake himalaya OK", flush=True)
        os.write(fd, b"\x1b")  # Esc: nothing pending → quit
        status, out = wait_exit(pid, fd)
        assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
            f"exit status {status!r}"
        )
        assert "\x1b[?1049l" in out, "terminal did not leave the alternate screen"
        assert "panicked at" not in out, "panic text leaked into the terminal"
        print("   exit 0 + alt-screen restore + no panic OK", flush=True)
    finally:
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError):
            pass
        os.close(fd)


def step_editor_validation(binpath, config):
    print("== editor validation: $EDITOR unset must fail startup ==", flush=True)
    pid, fd = spawn([binpath, config], {"EDITOR": None})
    try:
        status, out = wait_exit(pid, fd, timeout=8.0)
        text = strip_ansi(out)
        assert not os.WIFEXITED(status) or os.WEXITSTATUS(status) != 0, (
            f"startup should have failed, exited {os.WEXITSTATUS(status)}"
        )
        assert "editor" in text.lower(), f"no editor issue reported:\n{text[-2000:]}"
        print("   startup refused with an editor validation issue OK", flush=True)
    finally:
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError):
            pass
        os.close(fd)


def step_external_editor(binpath, config, editor_name, expect_import):
    label = "succeeding" if expect_import else "failing"
    print(f"== external editor ({label}): suspend → edit → resume ==", flush=True)
    pid, fd = spawn(
        [binpath, config],
        {
            "EDITOR": os.path.join(ROOT, "fixtures", "smoke", editor_name),
            "PATH": os.environ["PATH"],
        },
    )
    try:
        wait_for(fd, "Welcome")
        before = read_for(fd, 0.3)
        alt_screens = before.count("\x1b[?1049h")

        # Compose, then hand the body to the external editor (Ctrl+E).
        os.write(fd, b"c")
        time.sleep(0.6)
        os.write(fd, b"\x05")
        # The editor run suspends the TUI; wait for it to finish and the
        # app to repaint with the imported text (or the failure status).
        needle = "EDITED-BY-SMOKE" if expect_import else "External editor failed"
        end = time.time() + 12.0
        acc = ""
        while time.time() < end:
            acc += read_for(fd, 0.3)
            if needle in strip_ansi(acc):
                break
        assert needle in strip_ansi(acc), (
            f"editor flow did not reach {needle!r}:\n{strip_ansi(acc)[-3000:]}"
        )

        # The TUI re-entered the alternate screen after the editor (plan
        # §14 step 7), regardless of the editor's exit status.
        assert acc.count("\x1b[?1049h") > alt_screens, (
            "the alternate screen was not re-entered after the editor"
        )
        if expect_import:
            # Esc leaves the composer, second Esc quits.
            os.write(fd, b"\x1b")
            time.sleep(0.4)
            os.write(fd, b"\x1b")
            status, out = wait_exit(pid, fd)
            assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
                f"exit status {status!r}"
            )
            assert "\x1b[?1049l" in out, "terminal not restored after quit"
        else:
            # The composer stays usable: Esc leaves it, second Esc quits.
            os.write(fd, b"\x1b")
            time.sleep(0.4)
            os.write(fd, b"\x1b")
            status, out = wait_exit(pid, fd)
            assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
                f"exit status {status!r}"
            )
        print(f"   external editor ({label}) OK", flush=True)
    finally:
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError):
            pass
        os.close(fd)


def step_opener_environment():
    print("== opener environment: platform opener present ==", flush=True)
    program = "open" if sys.platform == "darwin" else "xdg-open"
    found = shutil.which(program)
    assert found, f"platform opener {program!r} missing from this runner"
    print(f"   {program} found at {found} OK", flush=True)


def wizard_type(fd, text):
    """Type text into the wizard one key at a time."""
    for ch in text:
        os.write(fd, ch.encode())
    time.sleep(0.2)


def drive_wizard_to_save(fd, already_on_email_screen=False):
    """Drive the wizard from the email screen to the saved screen.

    Uses TMAIL_FAKE_DISCOVERY=1 (Gmail candidate) and the fake himalaya
    for the credential test, so every step completes instantly. The
    needles are screen-unique strings that survive ratatui's diff-based
    redraw (unchanged cells are never retransmitted, so shared header
    prefixes must not be waited on).
    """
    if not already_on_email_screen:
        wait_for(fd, "Account setup — email address")
    print("   email screen rendered OK", flush=True)
    wizard_type(fd, "smoke@gmail.com")
    os.write(fd, b"\r")  # Enter: detect settings
    wait_for(fd, "imaps://imap.gmail.com:993")
    print("   discovery candidate rendered OK", flush=True)
    os.write(fd, b"\r")  # Enter: accept → identity
    wait_for(fd, "Name")
    print("   identity screen rendered OK", flush=True)
    os.write(fd, b"\r")  # Enter: continue → credentials
    wait_for(fd, "Password")
    print("   credentials screen rendered OK", flush=True)
    os.write(fd, b"\t\t")  # Tab, Tab: username → storage → password field
    os.write(fd, b"p")  # masked
    time.sleep(0.3)
    os.write(fd, b"\r")  # Enter: test connection
    wait_for(fd, "[accounts.gmail]")
    print("   credential test passed, confirm screen rendered OK", flush=True)
    os.write(fd, b"\r")  # Enter: save
    wait_for(fd, "Account saved to")


def step_wizard_manual(binpath, home):
    print(
        "== wizard (--configure): fake discovery → test → save → exit 0 ==", flush=True
    )
    pid, fd = spawn(
        [binpath, "--configure"],
        {
            "HOME": home,
            "TMAIL_FAKE_DISCOVERY": "1",
            "EDITOR": os.path.join(fake_dir(), "true-like"),
            "PATH": os.environ["PATH"],
        },
    )
    try:
        drive_wizard_to_save(fd)
        os.write(fd, b"\r")  # Enter: finish (manual mode exits)
        status, out = wait_exit(pid, fd)
        assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
            f"exit status {status!r}:\n{strip_ansi(out)[-2000:]}"
        )
        config = os.path.join(home, ".config/himalaya/config.toml")
        assert config in strip_ansi(out), (
            f"the saved path was not printed to stdout:\n{strip_ansi(out)[-2000:]}"
        )
        text = open(config).read()
        assert "[accounts.gmail]" in text, f"account block saved:\n{text}"
        assert 'imap.server = "imaps://imap.gmail.com:993"' in text, text
        assert 'imap.sasl.plain.password.raw = "p"' in text, text
        assert 'mailbox.alias.inbox = "INBOX"' in text, text
        mode = os.stat(config).st_mode & 0o777
        assert mode == 0o600, f"fresh config must be 0600, got {oct(mode)}"
        print(
            "   exit 0 + saved path on stdout + 0600 config with account OK", flush=True
        )
    finally:
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError):
            pass
        os.close(fd)


def step_wizard_first_run(binpath, home):
    print("== wizard (first run): auto-trigger → save → mailbox UI ==", flush=True)
    pid, fd = spawn(
        [binpath],
        {
            "HOME": home,
            "TMAIL_FAKE_DISCOVERY": "1",
            "EDITOR": os.path.join(fake_dir(), "true-like"),
            "PATH": os.environ["PATH"],
        },
    )
    try:
        # No config file exists under the temp HOME: the wizard starts by
        # itself instead of showing the fatal configuration screen.
        wait_for(fd, "Account setup — email address")
        print("   first-run auto trigger OK", flush=True)
        drive_wizard_to_save(fd, already_on_email_screen=True)
        os.write(fd, b"\r")  # Enter: finish (first-run continues into the app)
        # The session restarts into the normal mailbox UI: the fake
        # himalaya serves the canned inbox with the "Welcome" message.
        wait_for(fd, "Welcome")
        print("   mailbox UI rendered after the wizard OK", flush=True)
        os.write(fd, b"\x1b")  # Esc: quit
        status, out = wait_exit(pid, fd)
        assert os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, (
            f"exit status {status!r}"
        )
        assert "\x1b[?1049l" in out, "terminal not restored after quit"
        config = os.path.join(home, ".config/himalaya/config.toml")
        text = open(config).read()
        assert "[accounts.gmail]" in text, text
        mode = os.stat(config).st_mode & 0o777
        assert mode == 0o600, f"fresh config must be 0600, got {oct(mode)}"
        print("   exit 0 + 0600 config + clean quit OK", flush=True)
    finally:
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError):
            pass
        os.close(fd)


def fake_dir():
    return os.path.join(ROOT, "target", "smoke-bin")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin", required=True, help="path to the tmail binary")
    args = parser.parse_args()

    binpath = os.path.abspath(args.bin)
    assert os.path.isfile(binpath), f"binary not found: {binpath}"

    # A dedicated bin dir staged on PATH first: `himalaya` (symlink to the
    # committed fake) and `true-like` (symlink to the system true(1)), so
    # both the backend program and `$EDITOR` resolution exercise real PATH
    # lookup on both platforms — with no real himalaya installed.
    fake_dir = os.path.join(ROOT, "target", "smoke-bin")
    os.makedirs(fake_dir, exist_ok=True)
    real_true = shutil.which("true")
    assert real_true, "system `true` not found"
    fake = os.path.join(ROOT, "fixtures", "smoke", "fake_himalaya.sh")
    os.chmod(fake, 0o755)
    for name, target in (
        ("himalaya", fake),
        ("true-like", real_true),
    ):
        link = os.path.join(fake_dir, name)
        if os.path.lexists(link):
            os.remove(link)
        os.symlink(target, link)

    # The committed external editors (Phase 11 smoke): one saves the file,
    # one fails — both chmod'd executable here.
    for editor in ("fake_editor.sh", "failing_editor.sh"):
        os.chmod(os.path.join(ROOT, "fixtures", "smoke", editor), 0o755)

    # The fake himalaya wins over any installed binary; real tools stay
    # reachable behind it.
    env_path = fake_dir + os.pathsep + os.environ["PATH"]

    config = os.path.join(ROOT, "target", "smoke-config.toml")
    write_config(config)

    old_path = os.environ["PATH"]
    os.environ["PATH"] = env_path
    try:
        step_opener_environment()
        step_happy(binpath, config, fake_dir)
        step_editor_validation(binpath, config)
        step_external_editor(binpath, config, "fake_editor.sh", expect_import=True)
        step_external_editor(binpath, config, "failing_editor.sh", expect_import=False)
        # Wizard flows (ADR 0003 W7): isolated temp HOMEs so the save
        # target resolves to ~/.config/himalaya/config.toml under them.
        wizard_home_manual = tempfile.mkdtemp(prefix="tmail-wizard-home-")
        wizard_home_first = tempfile.mkdtemp(prefix="tmail-wizard-home-")
        step_wizard_manual(binpath, wizard_home_manual)
        step_wizard_first_run(binpath, wizard_home_first)
        shutil.rmtree(wizard_home_manual, ignore_errors=True)
        shutil.rmtree(wizard_home_first, ignore_errors=True)
    finally:
        os.environ["PATH"] = old_path
    print("SMOKE OK", flush=True)


if __name__ == "__main__":
    main()
