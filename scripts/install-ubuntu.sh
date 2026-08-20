#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEB="$ROOT/dist/cutting-board_0.1.0_all.deb"

if [[ ! -f "$DEB" ]]; then
  "$ROOT/scripts/build-deb.sh"
fi

if [[ -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  source /etc/os-release
  if [[ "${ID:-}" != "ubuntu" ]]; then
    echo "Warning: this installer is targeted at Ubuntu 24.04; detected ${PRETTY_NAME:-unknown Linux}." >&2
  elif [[ "${VERSION_ID:-}" != "24.04" ]]; then
    echo "Warning: Ubuntu 24.04 is the validated target; detected Ubuntu ${VERSION_ID:-unknown}." >&2
  fi
fi

sudo apt-get update
sudo apt-get install -y "$DEB"
echo "Installed. Launch 'Cutting Board' from the app menu or run: cutting-board"
