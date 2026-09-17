"""IMAP helpers shared by the E2E tests.

Tests arrange state and assert effects straight against GreenMail (the
same IMAP server the app talks to), so the assertions never trust the UI
about what was persisted.
"""

from __future__ import annotations

import imaplib
import time
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from datetime import datetime, timezone
from email.message import EmailMessage
from email.parser import BytesParser
from email.policy import default as default_policy
from email.utils import format_datetime, make_msgid, parsedate_to_datetime

HOST = "127.0.0.1"
PORT = 3143
USERNAME = "tmail"
PASSWORD = "tmailtmail"

ADDRESS = "tmail@testmailbox.com"

# The semantic mailboxes the E2E config aliases (tmail-e2e.toml). GreenMail
# exposes INBOX only, so the harness creates these once per session.
STANDARD_MAILBOXES = ("Trash", "Archive", "Drafts", "Sent")

# Background work (himalaya child processes) can lag the keystroke that
# started it; give every server-side wait a generous window.
SETTLE_TIMEOUT = 60.0


@contextmanager
def connection() -> Iterator[imaplib.IMAP4]:
    imap = imaplib.IMAP4(HOST, PORT, timeout=5)
    try:
        imap.login(USERNAME, PASSWORD)
        yield imap
    finally:
        try:
            imap.logout()
        except OSError:
            pass


def ensure_mailboxes(names: tuple[str, ...] = STANDARD_MAILBOXES) -> None:
    """Create the semantic mailboxes if missing (idempotent)."""
    with connection() as imap:
        for name in names:
            # CREATE fails once the mailbox exists; selectability is the
            # real contract either way, checked below.
            imap.create(name)
        for name in names:
            status, _ = imap.select(name, readonly=True)
            if status != "OK":
                raise RuntimeError(
                    f"mailbox {name!r} is not selectable; is GreenMail up?"
                )


def mailbox_count(mailbox: str) -> int:
    """The number of messages in `mailbox`."""
    with connection() as imap:
        status, data = imap.select(mailbox, readonly=True)
        if status != "OK":
            raise RuntimeError(f"mailbox {mailbox!r} is not selectable")
        count = data[0]
        assert count is not None
        return int(count)


def inbox_count() -> int:
    return mailbox_count("INBOX")


def uids(mailbox: str = "INBOX", criteria: str = "ALL") -> list[bytes]:
    """UIDs of the messages in `mailbox` matching the IMAP search `criteria`."""
    with connection() as imap:
        imap.select(mailbox, readonly=True)
        status, data = imap.uid("SEARCH", None, criteria)  # type: ignore[arg-type]
        if status != "OK":
            raise RuntimeError(f"UID SEARCH {criteria!r} failed in {mailbox!r}")
        return data[0].split()


def unseen_uids(mailbox: str = "INBOX") -> list[bytes]:
    return uids(mailbox, "UNSEEN")


def flagged_uids(mailbox: str = "INBOX") -> list[bytes]:
    return uids(mailbox, "FLAGGED")


def set_flag(ids: list[bytes], flag: str, add: bool) -> None:
    """Add or remove `flag` on every UID in `ids`."""
    if not ids:
        return
    operation = "+FLAGS.SILENT" if add else "-FLAGS.SILENT"
    sequence = b",".join(ids).decode("ascii")
    with connection() as imap:
        imap.select("INBOX")
        status, _ = imap.uid("STORE", sequence, operation, f"({flag})")
        if status != "OK":
            verb = "add" if add else "remove"
            raise RuntimeError(f"could not {verb} {flag} on {len(ids)} messages")


def clear_flag(ids: list[bytes], flag: str) -> None:
    set_flag(ids, flag, add=False)


def append_message(
    subject: str,
    mailbox: str = "INBOX",
    body: str | None = None,
    seen: bool = False,
) -> None:
    """Append one fresh, server-dated message to `mailbox`."""
    message = EmailMessage()
    message["From"] = "E2E Fixture <e2e@example.test>"
    message["To"] = ADDRESS
    message["Subject"] = subject
    message["Date"] = format_datetime(datetime.now(timezone.utc))
    message["Message-ID"] = make_msgid()
    message.set_content(body if body is not None else f"E2E fixture: {subject}")
    flags = r"(\Seen)" if seen else None
    with connection() as imap:
        status, response = imap.append(mailbox, flags, None, message.as_bytes())
        if status != "OK":
            raise RuntimeError(f"APPEND failed: {response!r}")


def delete_by_subject(subject: str, mailbox: str = "INBOX") -> None:
    """Permanently remove every message with `subject` from `mailbox`."""
    _delete_matching("Subject", subject, mailbox)


def delete_by_header(field: str, value: str, mailbox: str = "INBOX") -> None:
    """Permanently remove every message whose `field` header contains `value`."""
    _delete_matching(field, value, mailbox)


def _delete_matching(field: str, value: str, mailbox: str) -> None:
    with connection() as imap:
        imap.select(mailbox)
        status, data = imap.uid(
            "SEARCH",
            None,  # type: ignore[arg-type]
            "HEADER",
            field,
            f'"{value}"',
        )
        if status != "OK":
            raise RuntimeError(f"search for {field}: {value!r} failed in {mailbox!r}")
        ids = data[0].split()
        if not ids:
            return
        sequence = b",".join(ids).decode("ascii")
        imap.uid("STORE", sequence, "+FLAGS.SILENT", r"(\Deleted)")
        imap.expunge()


def message_headers(mailbox: str, fields: str = "SUBJECT DATE") -> list[dict[str, str]]:
    """Parse `fields` of every message in `mailbox` (UID fetch, no read side effects)."""
    with connection() as imap:
        imap.select(mailbox, readonly=True)
        status, data = imap.uid("SEARCH", None, "ALL")  # type: ignore[arg-type]
        if status != "OK" or not data[0].split():
            return []
        ids = b",".join(data[0].split()).decode("ascii")
        status, fetched = imap.uid(
            "FETCH", ids, f"(BODY.PEEK[HEADER.FIELDS ({fields})])"
        )
        if status != "OK":
            raise RuntimeError(f"UID FETCH headers failed in {mailbox!r}")
    parsed: list[dict[str, str]] = []
    for item in fetched:
        if not isinstance(item, tuple):
            continue
        message = BytesParser(policy=default_policy).parsebytes(item[1])
        parsed.append(
            {name.lower(): str(message[name] or "") for name in fields.split()}
        )
    return parsed


def subjects_by_date_desc(mailbox: str = "INBOX") -> list[str]:
    """Subjects in `mailbox`, newest `Date:` header first (himalaya's order)."""
    entries: list[tuple[datetime, str]] = []
    for headers in message_headers(mailbox):
        raw_date = headers.get("date", "")
        try:
            date = parsedate_to_datetime(raw_date)
        except (TypeError, ValueError):
            date = datetime.min
        if date.tzinfo is None:
            date = date.replace(tzinfo=timezone.utc)
        entries.append((date, headers.get("subject", "")))
    entries.sort(key=lambda entry: entry[0], reverse=True)
    return [subject for _, subject in entries]


def draft_recipients(mailbox: str = "Drafts") -> list[str]:
    """The raw `To` headers of every message in `mailbox`."""
    return [headers.get("to", "") for headers in message_headers(mailbox, "TO")]


def wait_until(
    predicate: Callable[[], bool],
    timeout: float = SETTLE_TIMEOUT,
    description: str | None = None,
) -> None:
    """Poll `predicate` until it holds, or fail naming the condition."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.5)
    raise AssertionError(
        f"condition not met within {timeout}s: {description or predicate}"
    )
