#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}"

if [[ -n "${PYTHON:-}" ]]; then
  PYTHON_BIN="$PYTHON"
elif [[ -x "$ROOT/.venv/bin/python" ]]; then
  PYTHON_BIN="$ROOT/.venv/bin/python"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  echo "No project Python environment found." >&2
  echo "Run ./scripts/install-macos.sh, or set PYTHON to a Python 3.10+ interpreter." >&2
  exit 1
else
  PYTHON_BIN="python3"
fi

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  echo "Python interpreter not found: $PYTHON_BIN" >&2
  exit 1
fi

if ! "$PYTHON_BIN" -c 'import sys; raise SystemExit(sys.version_info < (3, 10))'; then
  echo "Cutting Board requires Python 3.10 or newer: $PYTHON_BIN" >&2
  exit 1
fi

exec "$PYTHON_BIN" -m cutting_board "$@"
