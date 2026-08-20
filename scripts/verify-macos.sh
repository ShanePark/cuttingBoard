#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This verification script is for macOS only." >&2
  exit 1
fi

if [[ -n "${PYTHON:-}" ]]; then
  PYTHON_BIN="$PYTHON"
elif [[ -x "$ROOT/.venv/bin/python" ]]; then
  PYTHON_BIN="$ROOT/.venv/bin/python"
else
  echo "No project Python environment found." >&2
  echo "Run ./scripts/install-macos.sh, or set PYTHON to a Python 3.10+ interpreter with Tk." >&2
  exit 1
fi

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  echo "Python interpreter not found: $PYTHON_BIN" >&2
  exit 1
fi

TEMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEMP_ROOT"' EXIT
export PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}"
export PYTHONPYCACHEPREFIX="$TEMP_ROOT/pycache"

printf '\n[1/4] Check the macOS Python and Tk runtime\n'
"$PYTHON_BIN" - <<'PY_CHECK'
import sys
import tkinter

if sys.version_info < (3, 10):
    raise SystemExit(f"Python 3.10+ is required; found {sys.version.split()[0]}")
print(f"Python {sys.version.split()[0]} · Tk {tkinter.TkVersion} · {sys.executable}")
PY_CHECK

printf '\n[2/4] Run the unit and platform integration tests\n'
"$PYTHON_BIN" -W error::ResourceWarning -m unittest discover -s "$ROOT/tests" -v

printf '\n[3/4] Validate a live macOS JSON snapshot\n'
SNAPSHOT_FILE="$TEMP_ROOT/snapshot.json"
"$PYTHON_BIN" -m cutting_board --snapshot --json > "$SNAPSHOT_FILE" || {
  # A partial scan may report a warning and exit 1 while still producing a valid snapshot.
  test -s "$SNAPSHOT_FILE"
}
"$PYTHON_BIN" - "$SNAPSHOT_FILE" <<'PY_CHECK'
import json
import sys
from pathlib import Path

payload = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
required = {"scanned_at", "scan_duration_ms", "services", "current_uid", "errors"}
missing = sorted(required - payload.keys())
if missing:
    raise SystemExit(f"snapshot fields missing: {missing}")
if not isinstance(payload["services"], list):
    raise SystemExit("services is not a list")
print(f"snapshot schema OK ({len(payload['services'])} visible services)")
PY_CHECK

printf '\n[4/4] Run the native Tk GUI smoke tests\n'
if [[ "${SKIP_GUI:-0}" == "1" ]]; then
  echo "Skipped because SKIP_GUI=1."
else
  SETTINGS_FILE="$TEMP_ROOT/settings.json"
  "$PYTHON_BIN" -m cutting_board --demo --auto-close 1 --settings-file "$SETTINGS_FILE"
  "$PYTHON_BIN" -m cutting_board --auto-close 1 --settings-file "$SETTINGS_FILE"
fi

printf '\nAll macOS verification stages passed.\n'
