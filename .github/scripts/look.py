"""Measures a screenshot of a Sandhill window and prints GitHub notices: where
macOS drew the window buttons and the toolbar's first icon, in points, and
a small JPEG picture of its top left corner, base64 in pieces. This checks
the look on a real Mac from the check's annotations, which keep at most 4096
characters each and 10 notices a step.

    python look.py window.png WIDTH_IN_POINTS
"""

import base64
import io
import sys

from PIL import Image

PIECE = 4000


def notice(title: str, message: str) -> None:
    message = message.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    title = title.replace("%", "%25").replace(":", "%3A").replace(",", "%2C")
    print(f"::notice title={title}::{message}", flush=True)


def find(img: Image.Image, scale: float, box: tuple[int, int, int, int], match) -> str:
    """The bounding box, in points, of the pixels in `box` (points) that match."""
    x0, y0, x1, y1 = (round(v * scale) for v in box)
    px = img.load()
    xs, ys = [], []
    for y in range(y0, min(y1, img.height)):
        for x in range(x0, min(x1, img.width)):
            if match(*px[x, y]):
                xs.append(x)
                ys.append(y)
    if not xs:
        return "not found"
    left, right, top, bottom = min(xs) / scale, (max(xs) + 1) / scale, min(ys) / scale, (max(ys) + 1) / scale
    return (
        f"x {left:.1f}-{right:.1f} y {top:.1f}-{bottom:.1f} "
        f"centre ({(left + right) / 2:.1f}, {(top + bottom) / 2:.1f})"
    )


def picture(title: str, img: Image.Image, quality: int) -> None:
    out = io.BytesIO()
    img.convert("RGB").save(out, "JPEG", quality=quality)
    data = base64.b64encode(out.getvalue()).decode()
    pieces = [data[i : i + PIECE] for i in range(0, len(data), PIECE)]
    for i, piece in enumerate(pieces):
        notice(f"{title} {i + 1}/{len(pieces)} {img.width}x{img.height}", piece)


def main() -> None:
    img = Image.open(sys.argv[1]).convert("RGB")
    width = float(sys.argv[2])
    scale = img.width / width
    top_left = (0, 0, 140, 60)
    lights = {
        "red": find(img, scale, top_left, lambda r, g, b: r > 220 and 60 < g < 140 and 50 < b < 130),
        "yellow": find(img, scale, top_left, lambda r, g, b: r > 220 and 160 < g < 215 and b < 90),
        "green": find(img, scale, top_left, lambda r, g, b: r < 90 and g > 160 and b < 110),
        # The left pane button, the toolbar's first icon, drawn in the muted text colour.
        "first icon": find(img, scale, (66, 0, 106, 34), lambda r, g, b: max(r, g, b) < 140),
    }
    notice(
        "Window buttons",
        f"screenshot {img.width}x{img.height} px, scale {scale:.2f}; "
        + "; ".join(f"{name}: {where}" for name, where in lights.items()),
    )
    corner = img.crop((0, 0, round(400 * scale), round(70 * scale)))
    picture("Top left", corner.resize((400, 70)), 70)


if __name__ == "__main__":
    main()
