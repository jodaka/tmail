#!/usr/bin/env bash
# Full E2E run: GreenMail + seeding in Docker, tmail built and tested on the host.
# See README.md for the architecture and the host prerequisites.
set -euo pipefail

cd "$(dirname "$0")"

./mail-only.sh
./run-tests.sh
