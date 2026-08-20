from __future__ import annotations

import os
from pathlib import Path


PROJECT_ROOT = Path(SPECPATH).resolve().parents[1]
ICON_PATH = PROJECT_ROOT / "build" / "macos" / "CuttingBoard.icns"
APP_VERSION = os.environ.get("CUTTING_BOARD_VERSION", "0.0.0")

a = Analysis(
    [str(PROJECT_ROOT / "src" / "cutting_board" / "__main__.py")],
    pathex=[str(PROJECT_ROOT / "src")],
    binaries=[],
    datas=[(str(PROJECT_ROOT / "assets"), "assets")],
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[str(PROJECT_ROOT / "packaging" / "macos" / "pyi_runtime_hook.py")],
    excludes=[],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="Cutting Board",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=False,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon=str(ICON_PATH),
)

coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=False,
    name="Cutting Board",
)

app = BUNDLE(
    coll,
    name="Cutting Board.app",
    icon=str(ICON_PATH),
    bundle_identifier="io.github.shanepark.cutting-board",
    info_plist={
        "CFBundleDisplayName": "Cutting Board",
        "CFBundleName": "Cutting Board",
        "CFBundleShortVersionString": APP_VERSION,
        "CFBundleVersion": APP_VERSION,
        "LSApplicationCategoryType": "public.app-category.developer-tools",
        "NSHighResolutionCapable": True,
    },
)
