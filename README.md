# Tmail

A Gmail-inspired, keyboard-first terminal email client built in Rust with
Ratatui, backed by the [Himalaya CLI](https://pimalaya.org) for all mail
protocols, accounts, and credentials.

## AI disclaimer 
Initial planning was performed with ChatGPT. 99.9% of the code was written by GLM-5.3-Flash. 
Design mockups were created in Open Design using GLM-5.3-Flash.

## Quick start

```sh
tmail --configure            # set up your account in-app (will be started automatically if you don't have config)
tmail                        # start reading mail
```

See [docs/installation.md](docs/installation.md) for details.

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
