#!/usr/bin/env bash
# Build tmail on the host and run the E2E tests against the Docker mail stack.
# Assumes GreenMail is up and seeded (./mail-only.sh); run.sh does both.
set -euo pipefail

cd "$(dirname "$0")"

# Local overrides (TMAIL_EXPECT_TEXT, TMAIL_CMD, ...). Optional; missing file is fine.
if [ -f .env ]; then
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
fi

# tmail refuses to start without himalaya; fail early with an actionable message.
if ! command -v himalaya >/dev/null 2>&1; then
    echo "error: himalaya not found on PATH (tmail prerequisite; see docs/installation.md)" >&2
    exit 1
fi
himalaya --version

echo "==> Building tmail"
cargo build

# Test dependencies live in an isolated venv, created on first use.
if [ ! -x .venv/bin/python ]; then
    echo "==> Creating Python test venv (.venv)"
    python3 -m venv .venv
    .venv/bin/pip install --quiet -r requirements.e2e.txt
fi

# The harness always drives tmail against the E2E config (GreenMail, not a
# personal account). conftest.py falls back to the same default.
export TMAIL_CONFIG="${TMAIL_CONFIG:-$(pwd)/tmail-e2e.toml}"

echo "==> Running E2E tests"
exec .venv/bin/python -m pytest -q tests
