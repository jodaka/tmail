#!/usr/bin/env bash
# A failing external editor for the CI smoke (fixtures/smoke/ci_smoke.py):
# touches nothing and exits non-zero, so Post must import nothing and still
# restore the terminal (plan §14 step 7).
exit 3
