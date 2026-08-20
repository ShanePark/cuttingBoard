#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENV="$ROOT/.venv"
PYTHON="$VENV/bin/python"
BUILD_DIR="$ROOT/build/macos"
ICONSET="$BUILD_DIR/CuttingBoard.iconset"
ICON="$BUILD_DIR/CuttingBoard.icns"
APP="$ROOT/dist/Cutting Board.app"
SOURCE_ICON="$ROOT/assets/app-icon-source.png"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS is required to build the .app bundle." >&2
  exit 1
fi

for command in sips iconutil ditto shasum; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command not found: $command" >&2
    exit 1
  fi
done

if [[ ! -x "$PYTHON" ]]; then
  echo "No .venv found; preparing the Homebrew Python environment first."
  "$ROOT/scripts/install-macos.sh"
fi

"$PYTHON" -m pip install --disable-pip-version-check --editable "$ROOT[build]"
"$PYTHON" -c "import tkinter" >/dev/null || {
  echo "The Python interpreter used by .venv must include Tkinter." >&2
  exit 1
}

VERSION="$(PYTHONPATH="$ROOT/src" "$PYTHON" -c 'from cutting_board import __version__; print(__version__)')"
MACHINE="$(uname -m)"
case "$MACHINE" in
  arm64) ARCH="arm64" ;;
  x86_64) ARCH="x86_64" ;;
  *)
    echo "Unsupported macOS architecture: $MACHINE" >&2
    exit 1
    ;;
esac
ZIP="$ROOT/dist/Cutting-Board-$VERSION-macos-$ARCH.zip"

mkdir -p "$BUILD_DIR" "$ROOT/dist"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"

while read -r filename size; do
  sips -z "$size" "$size" "$SOURCE_ICON" --out "$ICONSET/$filename" >/dev/null
done <<'ICON_SIZES'
icon_16x16.png 16
icon_16x16@2x.png 32
icon_32x32.png 32
icon_32x32@2x.png 64
icon_128x128.png 128
icon_128x128@2x.png 256
icon_256x256.png 256
icon_256x256@2x.png 512
icon_512x512.png 512
icon_512x512@2x.png 1024
ICON_SIZES
if ! iconutil -c icns "$ICONSET" -o "$ICON"; then
  echo "iconutil rejected the iconset; using the compatible ICNS packer." >&2
  "$PYTHON" "$ROOT/packaging/macos/build_icns.py" "$ICONSET" "$ICON"
fi

rm -rf "$APP"
rm -f "$ZIP"
CUTTING_BOARD_VERSION="$VERSION" "$PYTHON" -m PyInstaller \
  --noconfirm \
  --clean \
  --distpath "$ROOT/dist" \
  --workpath "$BUILD_DIR/pyinstaller" \
  "$ROOT/packaging/macos/cutting-board.spec"

if [[ ! -d "$APP" ]]; then
  echo "PyInstaller did not create the expected app bundle: $APP" >&2
  exit 1
fi

ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
SHA256="$(shasum -a 256 "$ZIP" | awk '{print $1}')"

printf 'Application: %s\n' "$APP"
printf 'Archive:     %s\n' "$ZIP"
printf 'Version:     %s\n' "$VERSION"
printf 'Architecture: %s\n' "$ARCH"
printf 'SHA256:      %s\n' "$SHA256"
