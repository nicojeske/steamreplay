#!/usr/bin/env bash
# Runs every method on every dumped session (see README.md). Safe to re-run; outputs are overwritten.
set -euo pipefail
cd "$(dirname "$0")"
for m in panns effat ast; do uv run python taggers.py "$m"; done
uv run python clap.py
uv run python arousal.py
[ "${SKIP_VOCALS:-0}" = 1 ] || uv run python vocals.py
for m in panns effat ast; do uv run python taggers.py "$m" --input vocals; done
uv run python clap.py --input vocals
uv run python arousal.py --input vocals
