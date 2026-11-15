"""Makes the Maple Mono NF fonts the app bundles from Maple Mono's WOFF2 web
fonts (the retired Tauri app kept them in app/src/fonts; they are also in
Maple Mono's NF releases). GPUI's text systems read TrueType, not WOFF2, so
they are stored as gzipped TTF and unpacked when the app starts.

Needs fontTools and brotli: pip install fonttools brotli
Run from anywhere: python3 app/assets/fonts/make.py [folder with the .woff2 files]
"""

import gzip
import io
import sys
from pathlib import Path

from fontTools.ttLib import TTFont

HERE = Path(__file__).resolve().parent
SOURCE = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE
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
