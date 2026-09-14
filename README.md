# Tmail

A Gmail-inspired, keyboard-first terminal email client built in Rust with
Ratatui, backed by the [Himalaya CLI](https://pimalaya.org) for all mail
protocols, accounts, and credentials. Built using AI.

## Quick start

```sh
tmail --configure            # set up your account in-app
tmail                        # start reading mail
```

See [docs/installation.md](docs/installation.md) for Homebrew and
source installs plus requirements.

## Documentation

- [Installation](docs/installation.md) — Homebrew / from source, requirements, running
- [Configuration](docs/configuration.md) — the shared TOML file, all `[tmail]` options, theming, startup validation
- [Account wizard](docs/account-wizard.md) — in-app account setup and credential storage
- [Features](docs/features.md) — external editor, cache, bulk selection, search, mouse
- [Known limitations](docs/limitations.md) — v1 scope and backend-specific caveats
- [Development](docs/development.md) — module map, build/test commands, repository layout
- [Keyboard shortcuts](docs/shortcusts.md) — the default keybindings
- [Architecture](docs/architecture.md) — module layers and data flow

Config example: [`config.example.toml`](config.example.toml)
