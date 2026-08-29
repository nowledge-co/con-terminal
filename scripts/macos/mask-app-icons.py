#!/usr/bin/env python3
"""Bake a padded macOS-style squircle into selectable app icons.

`setApplicationIconImage` does not apply the Dock's usual optical inset.
Full-bleed squircles therefore look a size larger than bundled App Store
icons. The artwork is scaled to ~80% of the canvas (Apple's 824/1024
icon grid) and given a matching squircle mask.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageChops

SQUIRCLE_N = 5.0
ICON_SIZE = 512
# Apple's macOS 11+ icon shape is ~824px on a 1024 canvas.
ART_SCALE = 0.80


def squircle_alpha(size: int, n: float = SQUIRCLE_N, scale: float = ART_SCALE) -> list[int]:
    center = (size - 1) / 2.0
    aa = n / (center * scale)
    alpha = [0] * (size * size)
    for y in range(size):
        yn = abs(((y - center) / center) / scale) ** n
        row = y * size
        for x in range(size):
            xn = abs(((x - center) / center) / scale) ** n
            field = xn + yn
            alpha[row + x] = int(max(0.0, min(1.0, (1.0 - field) / aa + 0.5)) * 255)
    return alpha


def mask_icon(src: Path, dest: Path, mask: Image.Image) -> None:
    art_size = round(ICON_SIZE * ART_SCALE)
    art = Image.open(src).convert("RGBA").resize((art_size, art_size), Image.Resampling.LANCZOS)
    canvas = Image.new("RGBA", (ICON_SIZE, ICON_SIZE), (0, 0, 0, 0))
    offset = (ICON_SIZE - art_size) // 2
    canvas.paste(art, (offset, offset), art)
    r, g, b, a = canvas.split()
    a = ImageChops.multiply(a, mask)
    Image.merge("RGBA", (r, g, b, a)).save(dest, format="PNG", optimize=True)


def main() -> None:
    repo = Path(__file__).resolve().parents[2]
    dest_dir = repo / "assets" / "app-icons"
    dest_dir.mkdir(parents=True, exist_ok=True)

    sources = sorted((repo / ".context" / "ip-as-logo").glob("con-raccoon*.png"))
    if not sources:
        sources = sorted(dest_dir.glob("con-raccoon*.png"))
    if not sources:
        raise SystemExit("no raccoon icon sources found")

    mask = Image.new("L", (ICON_SIZE, ICON_SIZE))
    mask.putdata(squircle_alpha(ICON_SIZE))

    for src in sources:
        dest = dest_dir / src.name
        mask_icon(src, dest, mask)
        print(f"{src.name:32} {dest.stat().st_size / 1024:.0f}K")


if __name__ == "__main__":
    main()
