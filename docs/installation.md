# Installation

## Homebrew

Tap and install (the tap formula is synced from `Formula/tmail.rb` on every
release and pulls the prebuilt binaries from the GitHub releases):

```sh
brew tap jodaka/tap https://github.com/jodaka/homebrew-tap
brew install jodaka/tap/tmail   # installs himalaya automatically
```

## From source

Tmail itself is built from source on every platform; only the Himalaya CLI
dependency has platform-specific installers.

1. Install the Himalaya CLI (v2.x, with the backend feature you need):

   ```sh
   # macOS
   brew install himalaya

   # Linux
   pacman -S himalaya                                        # Arch
   dnf copr enable atim/himalaya && dnf install himalaya     # Fedora / RHEL / CentOS
   nix profile install github:pimalaya/himalaya              # Nix with flakes

   # any platform with Rust
   cargo install --locked --git https://github.com/pimalaya/himalaya.git

   # any Linux, prebuilt binary from GitHub releases
   curl -sSL https://raw.githubusercontent.com/pimalaya/himalaya/master/install.sh | PREFIX=~/.local sh
   ```

   The Homebrew and distro builds compile with himalaya's default features
   (IMAP + SMTP); `cargo install` lets you pick the backend feature you
   need. See the [himalaya docs](https://github.com/pimalaya/himalaya#installation).

2. Build Tmail from source:

   ```sh
   git clone <this repository>
   cd tmail
   cargo build --release
   ```

   The binary lands at `target/release/tmail`.

3. Run it (`cargo run --release` from the checkout, or copy the binary
   anywhere):

## Requirements

- macOS (required) / Linux (supported); Windows is out of scope
- Stable Rust (2024 edition)
- `himalaya` CLI v2.x installed and on `PATH` (Tmail checks at startup and
  refuses to start with an actionable error if it is missing)

## Running

```sh
cargo run --release                          # himalaya's default config
cargo run --release -- path/to/config.toml   # explicit config file
TMAIL_CONFIG=path/to/config.toml cargo run --release
tmail --configure                            # account setup wizard
tmail --debug                                # verbose logging to /tmp/tmail/log/tmail.log (daily rotation)
```

### Logging

By default a release build logs nothing. Debug builds trace at `debug`
level, and `tmail --debug` enables it in any build. Set `RUST_LOG` to
override the filter explicitly. All output goes to a rotating file
under `/tmp/tmail/log/` (never to the screen or the TUI).
