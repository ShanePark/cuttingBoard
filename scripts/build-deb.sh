#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(PYTHONPATH="$ROOT/src" python3 -c 'from cutting_board import __version__; print(__version__)')"
PACKAGE_NAME="cutting-board_${VERSION}_all"
TEMP_BUILD="$(mktemp -d)"
trap 'rm -rf "$TEMP_BUILD"' EXIT
BUILD_ROOT="$TEMP_BUILD/$PACKAGE_NAME"
DEB_PATH="$ROOT/dist/${PACKAGE_NAME}.deb"

mkdir -p \
  "$BUILD_ROOT/DEBIAN" \
  "$BUILD_ROOT/usr/bin" \
  "$BUILD_ROOT/usr/lib/cutting-board/cutting_board" \
  "$BUILD_ROOT/usr/share/applications" \
  "$BUILD_ROOT/usr/share/cutting-board/assets" \
  "$BUILD_ROOT/usr/share/icons/hicolor/32x32/apps" \
  "$BUILD_ROOT/usr/share/icons/hicolor/48x48/apps" \
  "$BUILD_ROOT/usr/share/icons/hicolor/64x64/apps" \
  "$BUILD_ROOT/usr/share/icons/hicolor/128x128/apps" \
  "$BUILD_ROOT/usr/share/icons/hicolor/256x256/apps" \
  "$BUILD_ROOT/usr/share/icons/hicolor/512x512/apps" \
  "$BUILD_ROOT/usr/share/doc/cutting-board"
chmod 0755 "$BUILD_ROOT" "$BUILD_ROOT/DEBIAN"

sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/debian/control.in" > "$BUILD_ROOT/DEBIAN/control"

cat > "$BUILD_ROOT/usr/bin/cutting-board" <<'EOF'
#!/bin/sh
set -eu
export PYTHONPATH="/usr/lib/cutting-board${PYTHONPATH:+:$PYTHONPATH}"
exec /usr/bin/python3 -m cutting_board "$@"
EOF
chmod 0755 "$BUILD_ROOT/usr/bin/cutting-board"

cp -a "$ROOT/src/cutting_board/." "$BUILD_ROOT/usr/lib/cutting-board/cutting_board/"
find "$BUILD_ROOT/usr/lib/cutting-board" -type d -name __pycache__ -prune -exec rm -rf {} +
find "$BUILD_ROOT/usr/lib/cutting-board" -type f -name '*.pyc' -delete

install -m 0644 "$ROOT/packaging/cutting-board.desktop" \
  "$BUILD_ROOT/usr/share/applications/cutting-board.desktop"
# The window icon plus every sized copy: the UI picks the one that fits its
# header, and the hicolor theme takes the rest for the shell.
install -m 0644 "$ROOT/assets/cutting-board.png" \
  "$BUILD_ROOT/usr/share/cutting-board/assets/cutting-board.png"
for size in 32 48 64 128 256 512; do
  install -m 0644 "$ROOT/assets/cutting-board-${size}.png" \
    "$BUILD_ROOT/usr/share/cutting-board/assets/cutting-board-${size}.png"
  install -m 0644 "$ROOT/assets/cutting-board-${size}.png" \
    "$BUILD_ROOT/usr/share/icons/hicolor/${size}x${size}/apps/cutting-board.png"
done

for document in README.md SPEC.md CONTRIBUTING.md CHANGELOG.md LICENSE TEST_REPORT.md; do
  if [[ -f "$ROOT/$document" ]]; then
    install -m 0644 "$ROOT/$document" "$BUILD_ROOT/usr/share/doc/cutting-board/$document"
  fi
done

gzip -9 -n -c "$ROOT/CHANGELOG.md" > "$BUILD_ROOT/usr/share/doc/cutting-board/changelog.gz" 2>/dev/null || true
find "$BUILD_ROOT" -type d -exec chmod g-s {} +
find "$BUILD_ROOT" -type d -exec chmod 0755 {} +

mkdir -p "$ROOT/dist"
rm -f "$DEB_PATH"
dpkg-deb --root-owner-group --build "$BUILD_ROOT" "$DEB_PATH"
echo "$DEB_PATH"
