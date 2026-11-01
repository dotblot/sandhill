"""Makes the Maple Mono NF fonts the GPUI app bundles, from the web fonts the
Tauri app uses (app/src/fonts). GPUI's text systems read TrueType, not WOFF2,
so they are stored as gzipped TTF and unpacked when the app starts.

Needs fontTools and brotli: pip install fonttools brotli
Run from anywhere: python3 gpui/assets/fonts/make.py
"""

import gzip
import io
from pathlib import Path

from fontTools.ttLib import TTFont

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[2] / "app" / "src" / "fonts"
WEIGHTS = ["Regular", "Italic", "SemiBold", "Bold"]

for weight in WEIGHTS:
    font = TTFont(SOURCE / f"MapleMono-NF-{weight}.woff2")
    font.flavor = None
    ttf = io.BytesIO()
    font.save(ttf)
    out = HERE / f"MapleMono-NF-{weight}.ttf.gz"
    # mtime=0 keeps the output the same from run to run.
    out.write_bytes(gzip.compress(ttf.getvalue(), compresslevel=9, mtime=0))
    print(out.name, out.stat().st_size)
