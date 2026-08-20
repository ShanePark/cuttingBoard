#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FORMULA="python-tk@3.13"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This installer is for macOS only." >&2
  exit 1
fi

if ! command -v brew >/dev/null 2>&1; then
  echo "Homebrew is required. Install it from https://brew.sh and run this script again." >&2
  exit 1
fi

if brew list --versions "$FORMULA" >/dev/null 2>&1; then
  echo "$FORMULA is already installed."
else
  echo "Installing $FORMULA with Homebrew..."
  brew install "$FORMULA"
fi

if [[ -n "${PYTHON:-}" ]]; then
  BASE_PYTHON="$PYTHON"
else
  BASE_PYTHON="$(brew --prefix python@3.13)/bin/python3.13"
fi

if ! command -v "$BASE_PYTHON" >/dev/null 2>&1; then
  echo "Python interpreter not found: $BASE_PYTHON" >&2
  exit 1
fi

if ! "$BASE_PYTHON" - <<'PY_CHECK'
import sys
import tkinter

if sys.version_info < (3, 10):
    raise SystemExit(f"Python 3.10+ is required; found {sys.version.split()[0]}")
print(f"Using Python {sys.version.split()[0]} with Tk {tkinter.TkVersion}")
PY_CHECK
then
  echo "Set PYTHON to a Python 3.10+ interpreter with Tk support, then try again." >&2
  exit 1
fi

BASE_PYTHON_VERSION="$($BASE_PYTHON -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')"

if [[ ! -x "$ROOT/.venv/bin/python" ]]; then
  echo "Creating $ROOT/.venv..."
  "$BASE_PYTHON" -m venv "$ROOT/.venv"
elif ! "$ROOT/.venv/bin/python" - "$BASE_PYTHON_VERSION" <<'PY_CHECK' >/dev/null 2>&1
import sys
import tkinter

expected = tuple(int(part) for part in sys.argv[1].split("."))
raise SystemExit(sys.version_info[:2] != expected)
PY_CHECK
then
  echo "Updating the existing .venv to Python $BASE_PYTHON_VERSION with Tk support..."
  "$BASE_PYTHON" -m venv --upgrade "$ROOT/.venv"
else
  echo "Reusing $ROOT/.venv."
fi

echo "Installing Cutting Board in editable mode..."
"$ROOT/.venv/bin/python" -m pip install -e "$ROOT"

"$ROOT/.venv/bin/python" - <<'PY_CHECK'
import sys
import tkinter
import cutting_board

print(f"Installed Cutting Board {cutting_board.__version__}")
print(f"Environment: {sys.executable} (Tk {tkinter.TkVersion})")
PY_CHECK

echo "Ready. Run: $ROOT/scripts/run.sh"
