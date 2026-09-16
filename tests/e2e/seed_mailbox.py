#!/usr/bin/env python3
"""Seed a GreenMail IMAP inbox with generated test messages.

Uses only the Python standard library.
"""

from __future__ import annotations

import argparse
import html
import imaplib
import os
import random
import socket
import time
from datetime import datetime, timedelta, timezone
from email import policy
from email.message import EmailMessage
from email.utils import format_datetime, make_msgid

DEFAULT_HOST = os.getenv("TMAIL_IMAP_HOST", "127.0.0.1")
DEFAULT_PORT = int(os.getenv("TMAIL_IMAP_PORT", "3143"))
DEFAULT_USERNAME = os.getenv("TMAIL_USERNAME", "tmail")
DEFAULT_PASSWORD = os.getenv("TMAIL_PASSWORD", "tmailtmail")
RECIPIENT = "tmail@testmailbox.com"

FIRST_NAMES = [
    "Alice", "Bob", "Carla", "Daniel", "Elena", "Farid", "Grace", "Hugo",
    "Iris", "Jonas", "Klara", "Liam", "Maya", "Noah", "Olivia", "Pavel",
    "Quinn", "Rina", "Sofia", "Theo", "Uma", "Victor", "Wendy", "Xavier",
    "Yara", "Zach",
]

LAST_NAMES = [
    "Adams", "Bennett", "Chen", "Diaz", "Evans", "Fischer", "Garcia",
    "Hansen", "Ivanov", "Jones", "Khan", "Lopez", "Miller", "Nakamura",
    "Owens", "Patel", "Rossi", "Singh", "Taylor", "Usman", "Vega",
    "Wilson", "Xu", "Young", "Zimmermann",
]

SUBJECTS = [
    "Quick question about {topic}",
    "Update: {topic}",
    "Following up on {topic}",
    "Notes from our {topic} discussion",
    "Can you review {topic}?",
    "Status of {topic}",
    "Next steps for {topic}",
    "Reminder: {topic}",
    "Thoughts on {topic}",
    "A small change to {topic}",
    "Meeting notes: {topic}",
    "FYI — {topic}",
]

TOPICS = [
    "the release", "the roadmap", "Q4 planning", "the design review",
    "the migration", "the customer demo", "the API changes", "the invoice",
    "the onboarding flow", "the test suite", "next week's meeting",
    "the documentation", "the deployment", "the prototype", "the report",
    "the contract", "the incident follow-up", "the new dashboard",
]

OPENERS = [
    "I wanted to send a quick update.",
    "Here are the latest details from my side.",
    "Thanks for the discussion earlier.",
    "I took another look and wrote down a few notes.",
    "A couple of things changed since the last message.",
    "I have a small question before we continue.",
]

DETAILS = [
    "The main work is complete, but there are still a few edge cases to check.",
    "The current version looks good and should be ready for another review.",
    "I noticed one issue that may affect the next release if we leave it as-is.",
    "The timeline still looks reasonable based on what we know today.",
    "There are two alternatives; the simpler one is probably sufficient for now.",
    "I added some notes and examples so the expected behavior is easier to verify.",
    "Nothing is blocked at the moment, although one dependency is still pending.",
    "The latest test run was mostly clean, with a few minor failures to investigate.",
    "We can keep the current approach unless new requirements appear.",
    "I would prefer to keep this change small and handle the broader cleanup separately.",
]

CLOSERS = [
    "Let me know what you think.",
    "Please send any comments when you have a chance.",
    "No action is needed unless you see something unexpected.",
    "I can adjust this based on your feedback.",
    "If this looks fine, I will proceed with the next step.",
    "Thanks — talk soon.",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Seed GreenMail with generated emails")
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--username", default=DEFAULT_USERNAME)
    parser.add_argument("--password", default=DEFAULT_PASSWORD)
    parser.add_argument("--count", type=int, default=100)
    parser.add_argument(
        "--seed",
        default="42",
        help="PRNG seed for reproducible mail. Use 'random' for a different mailbox each run.",
    )
    parser.add_argument(
        "--keep-existing",
        action="store_true",
        help="Append messages without deleting existing INBOX messages first.",
    )
    parser.add_argument(
        "--wait-seconds",
        type=float,
        default=20.0,
        help="How long to wait for IMAP to become available.",
    )
    return parser.parse_args()


def build_rng(seed_arg: str) -> random.Random:
    if seed_arg.lower() == "random":
        return random.Random()
    try:
        seed: object = int(seed_arg)
    except ValueError:
        seed = seed_arg
    return random.Random(seed)


def sender(rng: random.Random) -> tuple[str, str]:
    first = rng.choice(FIRST_NAMES)
    last = rng.choice(LAST_NAMES)
    name = f"{first} {last}"
    address = f"{first}.{last}{rng.randint(1, 999)}@example.test".lower()
    return name, address


def paragraphs(rng: random.Random) -> list[str]:
    result = [rng.choice(OPENERS)]
    result.extend(rng.sample(DETAILS, k=rng.randint(1, 3)))
    result.append(rng.choice(CLOSERS))
    return result


def build_message(index: int, rng: random.Random, now: datetime) -> tuple[bytes, datetime, bool, bool]:
    name, address = sender(rng)
    topic = rng.choice(TOPICS)
    subject = rng.choice(SUBJECTS).format(topic=topic)
    body_parts = paragraphs(rng)

    # Spread messages over the previous 90 days and vary their time of day.
    message_date = now - timedelta(
        days=rng.randint(0, 89),
        hours=rng.randint(0, 23),
        minutes=rng.randint(0, 59),
    )

    is_html = rng.random() < 0.4
    is_seen = rng.random() < 0.35

    msg = EmailMessage()
    msg["From"] = f"{name} <{address}>"
    msg["To"] = RECIPIENT
    msg["Subject"] = subject
    msg["Date"] = format_datetime(message_date)
    msg["Message-ID"] = make_msgid(idstring=f"seed-{index}", domain="example.test")
    msg["X-Test-Seed-Index"] = str(index)

    if is_html:
        html_paragraphs = "\n".join(f"<p>{html.escape(p)}</p>" for p in body_parts)
        msg.set_content(
            "<!doctype html>\n"
            "<html><body>\n"
            f"<h2>{html.escape(subject)}</h2>\n"
            f"{html_paragraphs}\n"
            "</body></html>\n",
            subtype="html",
        )
    else:
        msg.set_content("\n\n".join(body_parts))

    return msg.as_bytes(policy=policy.SMTP), message_date, is_html, is_seen


def connect_with_retry(host: str, port: int, username: str, password: str, timeout: float) -> imaplib.IMAP4:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None

    while time.monotonic() < deadline:
        try:
            imap = imaplib.IMAP4(host, port, timeout=3)
            imap.login(username, password)
            return imap
        except (OSError, socket.error, imaplib.IMAP4.error) as exc:
            last_error = exc
            time.sleep(0.5)

    raise RuntimeError(f"Could not connect/login to IMAP at {host}:{port}: {last_error}")


def clear_inbox(imap: imaplib.IMAP4) -> None:
    status, _ = imap.select("INBOX")
    if status != "OK":
        raise RuntimeError("Could not select INBOX")

    status, data = imap.search(None, "ALL")
    if status != "OK":
        raise RuntimeError("Could not list existing messages")

    ids = data[0].split()
    if ids:
        sequence_set = b",".join(ids).decode("ascii")
        status, _ = imap.store(sequence_set, "+FLAGS", "(\\Deleted)")
        if status != "OK":
            raise RuntimeError("Could not mark existing messages as deleted")
        imap.expunge()


def main() -> None:
    args = parse_args()
    if args.count < 1:
        raise SystemExit("--count must be at least 1")

    rng = build_rng(args.seed)
    now = datetime.now(timezone.utc)

    imap = connect_with_retry(
        args.host, args.port, args.username, args.password, args.wait_seconds
    )

    try:
        if not args.keep_existing:
            clear_inbox(imap)

        plain_count = 0
        html_count = 0
        seen_count = 0

        for index in range(1, args.count + 1):
            raw, message_date, is_html, is_seen = build_message(index, rng, now)
            flags = "(\\Seen)" if is_seen else None
            internal_date = imaplib.Time2Internaldate(message_date.timestamp())
            status, response = imap.append("INBOX", flags, internal_date, raw)
            if status != "OK":
                raise RuntimeError(f"APPEND failed for message {index}: {response!r}")

            html_count += int(is_html)
            plain_count += int(not is_html)
            seen_count += int(is_seen)

        status, data = imap.select("INBOX")
        mailbox_total = int(data[0]) if status == "OK" else -1

        print(f"Seeded {args.count} messages into {RECIPIENT}")
        print(f"  plain text: {plain_count}")
        print(f"  HTML:       {html_count}")
        print(f"  seen:       {seen_count}")
        print(f"  unread:     {args.count - seen_count}")
        print(f"  INBOX total: {mailbox_total}")
    finally:
        try:
            imap.logout()
        except Exception:
            pass


if __name__ == "__main__":
    main()
