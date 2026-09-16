#!/usr/bin/env bash
# Start GreenMail and seed INBOX (Docker only; tmail/tests run on the host).
set -euo pipefail

cd "$(dirname "$0")"

docker compose up -d greenmail
docker compose run --rm seed
