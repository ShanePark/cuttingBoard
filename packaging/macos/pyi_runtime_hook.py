from __future__ import annotations

import os
import sys
from pathlib import Path


bundle_root = Path(getattr(sys, "_MEIPASS", Path(sys.executable).parent))
assets_dir = bundle_root / "assets"
if assets_dir.is_dir():
    os.environ.setdefault("CUTTING_BOARD_ASSETS", str(assets_dir))
