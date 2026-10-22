#!/usr/bin/env python3
"""Build the Sandhill crane mark V1.1 (draft) from one geometry.

    python3 docs/design/ui/brand/build.py          # SVGs
    node docs/design/ui/brand/render.mjs           # PNGs from them (needs Playwright)

Every version shares the same paths; only the colours change. The paths were
drawn by hand on a 1024 canvas, using the V1.1 concept board as a guide for
the pose. Nothing in them is traced from, or cut out of, that image.
"""
from pathlib import Path

HERE = Path(__file__).resolve().parent
NAVY, CREAM, SAGE, RED = "#142844", "#F8F1E4", "#6FA38E", "#E64B3C"

# ---- the crane, flying right (units: the 1024 drawing canvas)
WING_BACK = "M108 295C220 330 380 380 455 422C522 462 556 518 568 575L574 600L452 612L445 598C290 555 160 455 108 295Z"
WING_FRONT = "M415 75C470 170 560 260 595 330C625 400 615 500 588 592L568 592L572 570C572 480 545 400 500 340C440 260 400 170 415 75Z"
BODY = "M445 598L580 575C660 572 702 520 742 465C757 441 774 425 798 421C824 412 852 403 886 398L962 381L882 409C864 412 853 416 843 422C805 450 780 505 742 565C700 630 625 678 540 695C500 703 460 706 410 709L292 737C342 702 396 640 445 598Z"
LEG_A = "M292 737L356 719C280 792 205 846 142 879C131 884 122 875 131 867C190 826 245 784 292 737Z"
LEG_B = "M410 702L462 694C370 768 270 846 184 902C174 908 165 899 173 891C252 828 340 760 410 702Z"
FEATHER = "M395 205C430 270 500 330 540 400C560 440 568 480 566 522C545 470 510 430 470 395C420 350 390 280 395 205Z"
CROWN = "M796 414C808 392 836 380 862 382C870 383 876 386 881 391C852 396 824 404 796 414Z"
# small sizes (64 px and below): one shorter, thicker leg and a slightly larger crown
SM_LEG = "M404 704L470 692C380 765 300 830 232 872C218 880 204 866 216 856C280 806 345 752 404 704Z"
SM_CROWN = "M788 418C800 386 834 371 864 374C873 375 880 380 885 387C854 393 822 403 788 418Z"
VB = (60, 16, 950, 950)  # the bird's box, centred, with a little room
CX, CY = VB[0] + VB[2] / 2, VB[1] + VB[3] / 2


def body(small=False):
    return " ".join([WING_BACK, WING_FRONT, BODY] + ([SM_LEG] if small else [LEG_A, LEG_B]))


def layers(crane, feather, crown, small=False, show_feather=True):
    out = f'<path fill="{crane}" d="{body(small)}"/>'
    if show_feather:
        out += f'<path fill="{feather}" d="{FEATHER}"/>'
    return out + f'<path fill="{crown}" d="{SM_CROWN if small else CROWN}"/>'


def mark(crane, feather, crown, bg=None, small=False, show_feather=True, size=None, title="Sandhill"):
    vb = " ".join(map(str, VB))
    wh = f' width="{size}" height="{size}"' if size else ""
    b = f'<rect x="{VB[0]}" y="{VB[1]}" width="{VB[2]}" height="{VB[3]}" fill="{bg}"/>' if bg else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}"{wh} role="img" aria-label="{title}">'
            f'<title>{title}</title>{b}{layers(crane, feather, crown, small, show_feather)}</svg>\n')


def app_icon(dark=True, small=False):
    """macOS app icon: 1024 canvas, 824 px rounded tile at 100 px (Apple's grid), crane centred on the tile."""
    s = 0.78 if small else 0.70
    tx, ty = 512 - CX * s, 512 - CY * s
    if dark:
        grad = '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1f3a5f"/><stop offset="1" stop-color="#142844"/></linearGradient>'
        crane = CREAM
    else:
        grad = '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fbf6ec"/><stop offset="1" stop-color="#f1e7d4"/></linearGradient>'
        crane = NAVY
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" role="img" aria-label="Sandhill"><title>Sandhill</title>'
            f'<defs>{grad}</defs><rect x="100" y="100" width="824" height="824" rx="185" fill="url(#g)"/>'
            f'<g transform="translate({tx:.1f} {ty:.1f}) scale({s})">{layers(crane, SAGE, RED, small, not small)}</g></svg>\n')


FILES = {
    # full mark
    "crane/sandhill-crane-light.svg": mark(NAVY, SAGE, RED, bg=CREAM),
    "crane/sandhill-crane-dark.svg": mark(CREAM, SAGE, RED, bg=NAVY),
    "crane/sandhill-crane-on-light.svg": mark(NAVY, SAGE, RED),  # transparent, for light surfaces
    "crane/sandhill-crane-on-dark.svg": mark(CREAM, SAGE, RED),  # transparent, for dark surfaces
    # one colour: the crown and feather keep their own shapes, separated from the body by gaps
    "crane/sandhill-crane-mono-navy.svg": mark(NAVY, NAVY, NAVY),
    "crane/sandhill-crane-mono-cream.svg": mark(CREAM, CREAM, CREAM),
    # macOS menu bar template: black on transparent; macOS recolours it
    "crane/sandhill-crane-template.svg": mark("#000", "#000", "#000", small=True, show_feather=False, size=18),
    # small mark
    "small/sandhill-mark-64.svg": mark(NAVY, SAGE, RED, small=True, size=64),
    "small/sandhill-mark-32.svg": mark(NAVY, SAGE, RED, small=True, size=32),
    "small/sandhill-mark-24.svg": mark(NAVY, SAGE, RED, small=True, size=24),
    "small/sandhill-mark-16.svg": mark(NAVY, SAGE, RED, small=True, show_feather=False, size=16),
    "small/sandhill-mark-16-dark.svg": mark(CREAM, SAGE, RED, small=True, show_feather=False, size=16),
    # app icon (drafts; not wired into the app)
    "app-icon/app-icon-dark.svg": app_icon(True),
    "app-icon/app-icon-dark-small.svg": app_icon(True, small=True),
    "app-icon/app-icon-light.svg": app_icon(False),
    "app-icon/app-icon-light-small.svg": app_icon(False, small=True),
}

if __name__ == "__main__":
    for rel, text in FILES.items():
        p = HERE / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    print(len(FILES), "SVGs written")
