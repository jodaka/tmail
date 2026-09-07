# Phase 0 Maildir probe environment
#
# Seeds a disposable Maildir tree under target/probe/maildir with representative
# messages used by the CLI probe and later contract tests.
#
# Usage: python3 fixtures/himalaya/seed_maildir.py

import mailbox
import pathlib
import time

ROOT = pathlib.Path(__file__).resolve().parents[2] / "target/probe/maildir"

HTML_BODY = """<html><body>
<h1>Rich message</h1>
<p>This is <b>bold</b> and <i>italic</i> with a <a href="https://example.org">link</a>.</p>
<ul><li>first</li><li>second</li></ul>
<blockquote>quoted text</blockquote>
<pre>code    keeps   spaces</pre>
</body></html>"""

# folder -> list of (subject, from, to, body(None => html alt), read, starred, extra_headers)
MESSAGES = {
    "INBOX": [
        (
            "Welcome to Tmail",
            "Ada Lovelace <ada@example.org>",
            "probe@tmail.local",
            "Hello,\n\nWelcome to Tmail, the terminal mail client.\n\n-- \nAda\n",
            False,
            True,
            {},
        ),
        (
            "Plain text only",
            "Bob <bob@example.org>",
            "probe@tmail.local",
            "This is a plain text message.\nLine two.\n",
            False,
            False,
            {},
        ),
        (
            "HTML alternative",
            "Carol <carol@example.org>",
            "probe@tmail.local",
            None,
            True,
            False,
            {},
        ),
        (
            "Grüße mit emoji 🎉",
            "Dave <dave@example.org>",
            "probe@tmail.local",
            "Unicode body: naïve, 中文, 🚀\n",
            False,
            False,
            {},
        ),
        (
            "Re: Welcome to Tmail",
            "Ada Lovelace <ada@example.org>",
            "probe@tmail.local",
            "Thanks for the welcome!\n",
            True,
            False,
            {},
        ),
    ],
    "Archive": [
        (
            "Old receipt",
            "Shop <shop@example.org>",
            "probe@tmail.local",
            "Your receipt #12345.\n",
            True,
            False,
            {},
        ),
    ],
    "Sent": [],
    "Drafts": [],
}


def build(subject: str, sender: str, to: str, body: str | None, extra: dict) -> bytes:
    from email.message import EmailMessage

    msg = EmailMessage()
    msg["Subject"] = subject
    msg["From"] = sender
    msg["To"] = to
    msg["Date"] = time.strftime("%a, %d %b %Y %H:%M:%S %z", time.gmtime())
    msg["Message-ID"] = f"<{abs(hash((subject, sender, body or '')))}@tmail.local>"
    for key, value in extra.items():
        msg[key] = value
    if body is None:
        msg.set_content("This message prefers HTML.\n")
        msg.add_alternative(HTML_BODY, subtype="html")
    else:
        msg.set_content(body)
    return msg.as_bytes()


def main() -> None:
    total = 0
    for folder, items in MESSAGES.items():
        box = mailbox.Maildir(ROOT / folder, create=True)
        for subject, sender, to, body, read, starred, extra in items:
            entry = mailbox.MaildirMessage(build(subject, sender, to, body, extra))
            flags = ""
            if read:
                flags += "S"
            if starred:
                flags += "F"
            if flags:
                entry.set_flags(flags)
            box.add(entry)
            total += 1
        box.flush()
        box.close()
        # Maildir convention: messages carrying flag info live in cur/.
        # Python's Maildir.flush() rewrites everything into new/, so we
        # relocate all files after the mailbox is closed.
        for path in (ROOT / folder / "new").iterdir():
            if path.is_file():
                path.rename(ROOT / folder / "cur" / path.name)
    print(f"Seeded {total} messages into {ROOT}")


if __name__ == "__main__":
    main()
