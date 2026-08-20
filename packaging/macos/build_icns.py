from __future__ import annotations

import argparse
import struct
from pathlib import Path


CHUNKS = (
    (b"icp4", "icon_16x16.png"),
    (b"icp5", "icon_32x32.png"),
    (b"icp6", "icon_32x32@2x.png"),
    (b"ic07", "icon_128x128.png"),
    (b"ic08", "icon_256x256.png"),
    (b"ic09", "icon_512x512.png"),
    (b"ic10", "icon_512x512@2x.png"),
)


def main() -> int:
    parser = argparse.ArgumentParser(description="Pack PNG icon representations into an ICNS file.")
    parser.add_argument("iconset", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    elements = []
    for kind, filename in CHUNKS:
        png = args.iconset.joinpath(filename).read_bytes()
        if not png.startswith(b"\x89PNG\r\n\x1a\n"):
            raise SystemExit(f"Not a PNG file: {args.iconset / filename}")
        elements.append(kind + struct.pack(">I", len(png) + 8) + png)

    body = b"".join(elements)
    args.output.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
