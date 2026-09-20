# Installation

## Prebuilt binaries

Grab `tmail-v<version>-<target>.tar.gz` from the GitHub releases, then
extract the binary next to somewhere on `PATH`:

- `aarch64-apple-darwin` / `x86_64-apple-darwin` — macOS
- `x86_64-unknown-linux-gnu` / `-musl` — Linux
- `x86_64-pc-windows-msvc` — Windows (`tar xzf`, then run `.\tmail.exe`;
  SmartScreen may warn about the unsigned binary on first launch)

`SHA256SUMS-v<version>.txt` carries the checksum of every asset.

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

   # Debian / Ubuntu (no distro package): the install script pulls the
   # right prebuilt.
   curl -sSL https://raw.githubusercontent.com/pimalaya/himalaya/master/install.sh \
     | sudo PREFIX=/usr/local sh
   # or without sudo:  ... | PREFIX=$HOME/.local sh   (needs ~/.local/bin on PATH)
   # or grab himalaya-v<version>.tar.gz from the pimalaya/himalaya releases
   # and move the binary to /usr/local/bin.

   # Windows
   # prebuilt x86_64 zip from the pimalaya/himalaya releases
   # (unzip himalaya-v*.zip and add it to PATH), or:

   # any platform with Rust
   cargo install --locked --git https://github.com/pimalaya/himalaya.git
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

4. Or grab a prebuilt binary from the GitHub releases instead of
   building: `tmail-v<version>-<target>.tar.gz` where target is
   `aarch64-apple-darwin` or `x86_64-apple-darwin` (macOS),
   `x86_64-unknown-linux-gnu` / `-musl` (Linux), or
   `x86_64-pc-windows-msvc` (Windows: unpack with `tar xzf`, then run
   `.\tmail.exe`; SmartScreen may warn about the unsigned binary on
   first launch). SHA checksums are published alongside (`SHA256SUMS`).

## Requirements

- macOS / Linux / Windows (all supported). Windows CI runs fmt, clippy,
  and the full test suite; the interactive pty smoke suite remains
  Unix-only, so end-to-end terminal behavior there is exercised by the
  test suite rather than a scripted terminal session.
- Stable Rust (2024 edition)
- `himalaya` CLI v2.x installed and on `PATH` (Tmail checks at startup and
  refuses to start with an actionable error if it is missing)

## Running

```sh
cargo run --release                          # himalaya's default config
cargo run --release -- path/to/config.toml   # explicit config file
TMAIL_CONFIG=path/to/config.toml cargo run --release
tmail --configure                            # account setup wizard
tmail --debug                                # verbose logging; all output
                                             # goes to a rotating file under
                                             # the system temp dir (on
                                             # macOS/Linux: /tmp/tmail/log,
                                             # on Windows: %TEMP%\tmail\log),
                                             # daily rotation
```

### Logging

By default a release build logs nothing. Debug builds trace at `debug`
level, and `tmail --debug` enables it in any build. Set `RUST_LOG` to
override the filter explicitly. All output goes to a rotating file
under `/tmp/tmail/log/` (never to the screen or the TUI).
