#!/usr/bin/env python3
"""Regenerates app icons and the UI logo from the repository's logo.png.

The source image has a faint, almost transparent halo; it is removed and the solid
interior made fully opaque before the Tauri icon set is generated.

    pip install pillow
    python3 scripts/icons.py
"""
import pathlib
import subprocess
import tempfile

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent.parent


def cleaned() -> Image.Image:
    im = Image.open(ROOT / "logo.png").convert("RGBA")
    r, g, b, a = im.split()
    a = a.point(lambda v: 0 if v < 40 else (255 if v >= 245 else int((v - 40) * 255 / 205)))
    im = Image.merge("RGBA", (r, g, b, a))
    return im.crop(a.getbbox())


def main() -> None:
    logo = cleaned()
    w, h = logo.size
    side = int(max(w, h) / 0.86)
    square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    square.paste(logo, ((side - w) // 2, (side - h) // 2))
    square = square.resize((1024, 1024), Image.LANCZOS)

    with tempfile.TemporaryDirectory() as tmp:
        src = pathlib.Path(tmp) / "icon.png"
        square.save(src)
        subprocess.run(
            ["npx", "tauri", "icon", str(src), "-o", str(ROOT / "desktop" / "icons")],
            check=True,
            cwd=ROOT,
        )
    for extra in ("android", "ios"):
        subprocess.run(["rm", "-rf", str(ROOT / "desktop" / "icons" / extra)], check=True)
    for f in (ROOT / "desktop" / "icons").glob("*Logo.png"):
        f.unlink()

    ui_logo = logo.copy()
    ui_logo.thumbnail((96, 96), Image.LANCZOS)
    ui_logo.save(ROOT / "ui" / "src" / "assets" / "logo.png", optimize=True)


if __name__ == "__main__":
    main()
