#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}"
PYTHON="${PYTHON:-python3}"
PYCACHE_ROOT="$(mktemp -d)"
export PYTHONPYCACHEPREFIX="$PYCACHE_ROOT"
find "$ROOT/src" "$ROOT/tests" -type d -name __pycache__ -prune -exec rm -rf {} +
find "$ROOT/src" "$ROOT/tests" -type f \( -name '*.pyc' -o -name '*.pyo' \) -delete

printf '\n[1/7] Compile all Python modules\n'
"$PYTHON" -m compileall -q "$ROOT/src" "$ROOT/tests"
"$PYTHON" - "$ROOT" <<'PY_COMPAT'
import ast
import sys
from pathlib import Path
root = Path(sys.argv[1])
for path in [*root.joinpath("src").rglob("*.py"), *root.joinpath("tests").rglob("*.py")]:
    ast.parse(path.read_text(encoding="utf-8"), filename=str(path), feature_version=(3, 10))
print("Python 3.10+ syntax compatibility OK")
PY_COMPAT

printf '\n[2/7] Run unit and live integration tests\n'
"$PYTHON" -W error::ResourceWarning -m unittest discover -s "$ROOT/tests" -v

printf '\n[3/7] Validate the headless JSON snapshot\n'
SNAPSHOT_FILE="$(mktemp)"
trap 'rm -f "$SNAPSHOT_FILE"; rm -rf "${EXTRACT_ROOT:-}" "$PYCACHE_ROOT"' EXIT
"$PYTHON" -m cutting_board --snapshot --json > "$SNAPSHOT_FILE" || {
  # Restricted containers may report a non-fatal scanner warning and exit 1.
  test -s "$SNAPSHOT_FILE"
}
"$PYTHON" - "$SNAPSHOT_FILE" <<'PY'
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
PY

# The GUI smoke test needs a display. Xvfb is the reproducible choice, but a
# developer running this on their own desktop already has one, and refusing to
# use it would make the whole script unrunnable for them.
if command -v xvfb-run >/dev/null 2>&1; then
  GUI_RUNNER=(xvfb-run -a)
  GUI_DISPLAY="Xvfb"
elif [ -n "${DISPLAY:-}" ] && command -v xdpyinfo >/dev/null 2>&1 && xdpyinfo >/dev/null 2>&1; then
  GUI_RUNNER=(env)
  GUI_DISPLAY="\$DISPLAY ($DISPLAY)"
else
  echo "no display available: install xvfb, or run with a reachable \$DISPLAY" >&2
  exit 1
fi

gui_run() {
  local settings
  settings="$(mktemp)"
  rm -f "$settings"
  "${GUI_RUNNER[@]}" "$@" --auto-close 1 --settings-file "$settings"
  rm -f "$settings"
}

printf '\n[4/7] Run the GUI smoke test on %s\n' "$GUI_DISPLAY"
gui_run "$PYTHON" -m cutting_board --demo
gui_run "$PYTHON" -m cutting_board

printf '\n[5/7] Build the Ubuntu .deb\n'
DEB_PATH="$($ROOT/scripts/build-deb.sh | tail -n 1)"
test -s "$DEB_PATH"
dpkg-deb --info "$DEB_PATH" >/dev/null
CONTENTS_FILE="$(mktemp)"
dpkg-deb --contents "$DEB_PATH" > "$CONTENTS_FILE"
grep -q './usr/bin/cutting-board' "$CONTENTS_FILE"
grep -q './usr/share/applications/cutting-board.desktop' "$CONTENTS_FILE"
grep -q './usr/share/doc/cutting-board/README.md' "$CONTENTS_FILE"
grep -q './usr/share/doc/cutting-board/SPEC.md' "$CONTENTS_FILE"
if grep -q '^drwxr-s' "$CONTENTS_FILE"; then
  echo "setgid directory leaked into package" >&2
  exit 1
fi
rm -f "$CONTENTS_FILE"

printf '\n[6/7] Smoke-test the exact files stored in the .deb\n'
EXTRACT_ROOT="$(mktemp -d)"
dpkg-deb -x "$DEB_PATH" "$EXTRACT_ROOT"
PYTHONPATH="$EXTRACT_ROOT/usr/lib/cutting-board" \
  CUTTING_BOARD_ASSETS="$EXTRACT_ROOT/usr/share/cutting-board/assets" \
  "$PYTHON" -m cutting_board --version | grep -q 'Cutting Board 0.1.0'
gui_run env \
  PYTHONPATH="$EXTRACT_ROOT/usr/lib/cutting-board" \
  CUTTING_BOARD_ASSETS="$EXTRACT_ROOT/usr/share/cutting-board/assets" \
  "$PYTHON" -m cutting_board --demo

printf '\n[7/7] Verify source hygiene\n'
if find "$ROOT/src" "$ROOT/tests" -type f \( -name '*.pyc' -o -name '*.pyo' \) | grep -q .; then
  echo "compiled artifacts found in source tree" >&2
  exit 1
fi
if grep -RInE '(TODO|FIXME|example\.invalid)' "$ROOT/src" "$ROOT/tests" "$ROOT/README.md" "$ROOT/SPEC.md" 2>/dev/null; then
  echo "unfinished markers found" >&2
  exit 1
fi

printf '\nAll verification stages passed.\n'
