#!/usr/bin/env python3
"""Build the Sandhill crane mark V1.1 (draft) from one geometry.

    python3 docs/design/ui/brand/build.py          # SVGs
    node docs/design/ui/brand/render.mjs           # PNGs from them (needs Playwright)

Every version shares the same paths; only the colours change. The paths were
drawn by hand on a 1024 canvas, using the V1.1 concept board as a guide for
the pose. Nothing in them is traced from, or cut out of, that image.
"""
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
# Navy, and the app's own colours beside it (Huan, 2026-11-14): Xcode's teal
# and red, as the file icons use, on a neutral white instead of cream.
NAVY, WHITE, TEAL, RED = "#142844", "#F5F5F7", "#67B7A4", "#FC6A5D"

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
        crane = WHITE
    else:
        grad = '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#ececf0"/></linearGradient>'
        crane = NAVY
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" role="img" aria-label="Sandhill"><title>Sandhill</title>'
            f'<defs>{grad}</defs><rect x="100" y="100" width="824" height="824" rx="185" fill="url(#g)"/>'
            f'<g transform="translate({tx:.1f} {ty:.1f}) scale({s})">{layers(crane, TEAL, RED, small, not small)}</g></svg>\n')


def srgb(hex_color):
    h = hex_color.lstrip("#")
    return "srgb:" + ",".join(f"{int(h[i:i + 2], 16) / 255:.5f}" for i in (0, 2, 4)) + ",1.00000"


def icon_package():
    """The app icon as an Icon Composer document (macOS 26 and later): the
    system draws it in the Dock with a white tile and navy crane in light
    mode, a navy tile and white crane in dark, and tints or clears it as the
    user's icon style says (Huan, 2026-11-14: flat, white tile in light).
    Each part of the crane is its own layer so its colour can change; the
    1024 canvas is the whole tile, which the system shapes and shades."""
    s = 0.70 * 1024 / 824  # the crane's size on the tile, as in app_icon()
    tx, ty = 512 - CX * s, 512 - CY * s

    def art(d):
        return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">'
                f'<g transform="translate({tx:.2f} {ty:.2f}) scale({s:.5f})"><path fill="#000" d="{d}"/></g></svg>\n')

    def layer(name, base, specialized=()):
        entry = {"name": name, "image-name": f"{name}.svg", "glass": False}
        if specialized:
            entry["fill-specializations"] = [{"value": {"solid": srgb(base)}}] + [
                {"appearance": appearance, "value": {"solid": srgb(color)}} for appearance, color in specialized
            ]
        else:
            entry["fill"] = {"solid": srgb(base)}
        return entry

    document = {
        "fill-specializations": [
            {"value": {"linear-gradient": [srgb("#FFFFFF"), srgb("#ECECF0")]}},
            {"appearance": "dark", "value": {"linear-gradient": [srgb("#1F3A5F"), srgb(NAVY)]}},
        ],
        "groups": [{
            # Front to back.
            "layers": [
                layer("crown", RED),
                layer("feather", TEAL),
                # Tinted icons are drawn from brightness: a navy crane would
                # vanish into the tint, so it is white there as in dark.
                layer("crane", NAVY, [("dark", WHITE), ("tinted", WHITE)]),
            ],
            "shadow": {"kind": "neutral", "opacity": 0.35},
            "translucency": {"enabled": False, "value": 0.5},
            "specular": False,
        }],
        "supported-platforms": {"squares": ["macOS"]},
    }
    return {
        "icon.json": json.dumps(document, indent=2, sort_keys=True) + "\n",
        "Assets/crane.svg": art(body()),
        "Assets/feather.svg": art(FEATHER),
        "Assets/crown.svg": art(CROWN),
    }


FILES = {
    # full mark
    "crane/sandhill-crane-light.svg": mark(NAVY, TEAL, RED, bg=WHITE),
    "crane/sandhill-crane-dark.svg": mark(WHITE, TEAL, RED, bg=NAVY),
    "crane/sandhill-crane-on-light.svg": mark(NAVY, TEAL, RED),  # transparent, for light surfaces
    "crane/sandhill-crane-on-dark.svg": mark(WHITE, TEAL, RED),  # transparent, for dark surfaces
    # one colour: the crown and feather keep their own shapes, separated from the body by gaps
    "crane/sandhill-crane-mono-navy.svg": mark(NAVY, NAVY, NAVY),
    "crane/sandhill-crane-mono-white.svg": mark(WHITE, WHITE, WHITE),
    # macOS menu bar template: black on transparent; macOS recolours it
    "crane/sandhill-crane-template.svg": mark("#000", "#000", "#000", small=True, show_feather=False, size=18),
    # small mark
    "small/sandhill-mark-64.svg": mark(NAVY, TEAL, RED, small=True, size=64),
    "small/sandhill-mark-32.svg": mark(NAVY, TEAL, RED, small=True, size=32),
    "small/sandhill-mark-24.svg": mark(NAVY, TEAL, RED, small=True, size=24),
    "small/sandhill-mark-16.svg": mark(NAVY, TEAL, RED, small=True, show_feather=False, size=16),
    "small/sandhill-mark-16-dark.svg": mark(WHITE, TEAL, RED, small=True, show_feather=False, size=16),
    # app icon (the dark one is the app's)
    "app-icon/app-icon-dark.svg": app_icon(True),
    "app-icon/app-icon-dark-small.svg": app_icon(True, small=True),
    "app-icon/app-icon-light.svg": app_icon(False),
    "app-icon/app-icon-light-small.svg": app_icon(False, small=True),
}

# the app's icon sources; app/assets/icon/make.mjs makes the bundle icon from them
APP_ICONS = HERE.parents[3] / "app" / "assets" / "icon"
APP_FILES = {"source.svg": FILES["app-icon/app-icon-dark.svg"], "source-small.svg": FILES["app-icon/app-icon-dark-small.svg"]}
APP_FILES.update({f"Sandhill.icon/{rel}": text for rel, text in icon_package().items()})

if __name__ == "__main__":
    for rel, text in FILES.items():
        p = HERE / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    for name, text in APP_FILES.items():
        p = APP_ICONS / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    print(len(FILES), "SVGs written, and", len(APP_FILES), "app icon sources")
