"""Renders the tether app icon (assets/icon.png + assets/tether.ico).

The mark matches `ui::sidebar::logo`: two linked rounded diamonds, green and blue,
on a navy rounded square. Run from the repo root: `python assets/make_icon.py`
(needs Pillow).
"""

from pathlib import Path

from PIL import Image, ImageDraw

SS = 1024  # supersampled canvas; every size is downscaled from it
BG = (0x14, 0x17, 0x29, 255)
BORDER = (0x2a, 0x2f, 0x4a, 255)
GREEN = (0x21, 0xB5, 0x68)
BLUE = (0x44, 0xA7, 0xFF)
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
OUT = Path(__file__).resolve().parent


def diamond(cx, cy, r):
    return [(cx, cy - r), (cx + r, cy), (cx, cy + r), (cx - r, cy)]


def render() -> Image.Image:
    img = Image.new("RGBA", (SS, SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    pad = SS * 0.04
    d.rounded_rectangle([pad, pad, SS - pad, SS - pad], radius=SS * 0.22, fill=BG, outline=BORDER, width=int(SS * 0.012))
    c = SS / 2
    r = SS * 0.25
    stroke = int(SS * 0.055)
    # Fills first (translucent), then strokes, so the link reads as interlocked.
    layer = Image.new("RGBA", (SS, SS), (0, 0, 0, 0))
    ld = ImageDraw.Draw(layer)
    for dx, col in [(-SS * 0.11, GREEN), (SS * 0.11, BLUE)]:
        ld.polygon(diamond(c + dx, c, r), fill=col + (60,))
    img = Image.alpha_composite(img, layer)
    d = ImageDraw.Draw(img)
    for dx, col in [(-SS * 0.11, GREEN), (SS * 0.11, BLUE)]:
        pts = diamond(c + dx, c, r)
        d.line(pts + [pts[0]], fill=col + (255,), width=stroke, joint="curve")
        for x, y in pts:  # round the corners
            d.ellipse([x - stroke / 2, y - stroke / 2, x + stroke / 2, y + stroke / 2], fill=col + (255,))
    return img


def main():
    big = render()
    big.resize((512, 512), Image.LANCZOS).save(OUT / "icon.png")
    frames = [big.resize((s, s), Image.LANCZOS) for s in ICO_SIZES]
    frames[-1].save(OUT / "tether.ico", sizes=[(s, s) for s in ICO_SIZES], append_images=frames[:-1])


if __name__ == "__main__":
    main()
