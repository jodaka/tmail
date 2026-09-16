# TMail E2E stack

Docker runs only the mail stack: **GreenMail SMTP + IMAP** and a mailbox
seeder that creates 12 random messages (one auto-sized page: select-all
covers the whole inbox deterministically). Tmail itself is built and run on
the **host**, driven by `pytest` + `pexpect` through a real PTY.

## Host prerequisites

- Docker with Docker Compose (GreenMail + seeder)
- Rust toolchain (`cargo build`)
- `himalaya` on `PATH` — tmail's hard prerequisite, version 2.1.0 is the
  tested CLI (`brew install himalaya` or `cargo install himalaya@2.1.0`)
- Python 3 (`python3 -m venv` available)

## Run everything

```sh
cd tests/e2e
./run.sh
```

`run.sh` will:

1. start GreenMail (IMAP on `127.0.0.1:3143`, SMTP on `127.0.0.1:3025`);
2. seed `INBOX` with 12 random plain-text/HTML messages;
3. build tmail with `cargo build` on the host;
4. create `.venv` with `pytest` + `pexpect` on first use;
5. run `tests/` through the PTY harness.

`run-tests.sh` alone repeats steps 3–5 against an already-running stack
(see `mail-only.sh` below).

## The E2E config

`tests/e2e/tmail-e2e.toml` is the **one file backing tmail and himalaya**
(the same shape the account wizard writes): the himalaya account block
points at GreenMail, the `[tmail]` table tunes the app (background refresh
off). The harness pins `TMAIL_CONFIG` to this file, so tmail always runs
against GreenMail — never your personal config.

Account (matches GreenMail's `-Dgreenmail.users` in `docker-compose.yml`):

```text
Email:       tmail@testmailbox.com
Username:    tmail
Password:    tmailtmail

IMAP host:   127.0.0.1:3143  (no TLS)
SMTP host:   127.0.0.1:3025  (no TLS)
```

From the host, GreenMail is also reachable directly:

```text
IMAP: 127.0.0.1:3143
SMTP: 127.0.0.1:3025
```

## Tests

`tests/conftest.py` provides a `TuiApp` fixture backed by `pexpect`:

```python
def test_open_message(app):
    app.expect("INBOX")
    app.press("enter")
    app.expect("message body text")
```

The included smoke test only requires that the application remains alive
after startup. Set this in `.env` (copy from `.env.example`) for an
additional startup assertion:

```text
TMAIL_EXPECT_TEXT=Inbox
```

The sidebar shows mailbox names as the server reports them; GreenMail
names its INBOX `Inbox`.

## Start only GreenMail + seed mailbox

```sh
./mail-only.sh
```

## Reseed

```sh
./reseed.sh
```

## Stop

```sh
./stop.sh
```
