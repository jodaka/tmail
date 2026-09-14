# Installation

## Homebrew

Tap and install (the tap formula is synced from `Formula/tmail.rb` on every
release and pulls the prebuilt binaries from the GitHub releases):

```sh
brew tap jodaka/tap https://github.com/jodaka/homebrew-tap
brew install jodaka/tap/tmail   # installs himalaya automatically
```

## From source

1. Install the Himalaya CLI (v2.x, with the backend feature you need):

   ```sh
   brew install himalaya        # macOS (Homebrew)
   cargo install himalaya       # any platform with Rust
   ```

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
```
