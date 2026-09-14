# Account configuration wizard

Tmail needs a working himalaya account to be useful. Instead of
hand-editing the config file, the built-in wizard sets one up in the app:

```sh
tmail --configure            # open the wizard at any time (optionally: --configure path/to/config.toml)
```

It also starts automatically on first run, when no usable `[accounts]`
entry is found. The flow:

1. **Email address** — the address you are configuring.
2. **Server settings** — Tmail discovers IMAP/SMTP endpoints for the
   domain in-process (Mozilla Thunderbird autoconfig, PACC, DNS SRV
   RFC 6186, fixed rules for Gmail/Outlook) and shows a ranked list. POP
   results are discarded (himalaya has no POP3 backend). If nothing is
   found — or the suggestion is wrong — press `e` and enter the servers
   manually (`imaps://host:993` style).
3. **Identity** — an optional display name.
4. **Sign-in** — a username plus your choice of password storage: either
   store the password in the config file (`password.raw`, file mode
   `0600`) or store a command that prints it (`password.cmd`, e.g.
   `pass show mail/gmail`). Tmail never executes that command — himalaya
   does, at connection time.
5. **Test** — the credentials are verified with a real `himalaya mailbox
   list` against a temporary owner-only config file. Nothing containing
   the credential is written to the real config until the test passes.
6. **Aliases** — the special-folder roles (`inbox`, `sent`, `drafts`,
   `trash`, `archive`) are derived from the tested mailbox listing (a
   Gmail preset plus generic name heuristics), shown for confirmation,
   and saved.

The account is merged into the shared config file with
format-preserving edits: existing accounts, `[tmail]` tables, comments
and ordering survive. A freshly created config file gets mode `0600`.
`Esc` steps back at every point and `Ctrl+C` quits; in `--configure`
mode the saved path is printed on success (exit 0) and
`tmail: configuration not changed` on cancel (exit 1).

**Gmail note:** IMAP with a normal Google password requires an **app
password** (a Google account with 2FA). The wizard shows this hint on
the sign-in screen.

**Privacy note:** discovery queries public infrastructure — the
Thunderbird ISPDB, `autoconfig` well-known URLs, DNS resolvers — and
therefore reveals the *domain* (and in some autoconfig query strings the
full address) to those services. Discovery only runs when you submit the
email screen; tests and smoke runs never touch the network
(`TMAIL_FAKE_DISCOVERY=1` selects a canned fake discoverer).
