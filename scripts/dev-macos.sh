#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "dev-macos.sh is intended for macOS." >&2
  exit 1
fi

PYTHON_BIN="$ROOT/.venv/bin/python"
if [[ ! -x "$PYTHON_BIN" ]]; then
  echo "No project Python environment found." >&2
  echo "Run ./scripts/install-macos.sh first." >&2
  exit 1
fi

exec "$PYTHON_BIN" "$ROOT/scripts/dev.py" "$@"
