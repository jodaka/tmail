#!/usr/bin/env bash
# A fake external editor for the CI smoke (fixtures/smoke/ci_smoke.py):
# appends a marker line to the file it was handed (the draft body), like a
# real editor session that writes and saves. Exit 0.
printf 'EDITED-BY-SMOKE\n' >> "$1"
