#!/usr/bin/env python3
"""Build Sandhill's shared icons from pinned, vendored upstream SVGs.

Run from any directory: python3 docs/design/ui/icons/build.py
No network, npm package, JetBrains plugin or font is needed. Edit catalog.json
for labels/lookup rules and *-sources.json for artwork selection. Original
SVGs and licenses live under vendor/. The crane/app icon is separate.
"""
import copy
import html
import json
import re
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
MOCKUP = HERE.parent / "sandhill-layout.html"
NS = "http://www.w3.org/2000/svg"
ET.register_namespace("", NS)

# Palette (Huan, 2026-11-14): Gruvbox Material's warm, earthy colours did not
# sit with Sandhill's cool glass, navy accent and Xcode code colours. File
# and folder icons from Gruvbox, and Sandhill's drawings in its style, keep
# their shapes and are repainted: each Gruvbox hue family becomes the
# matching Xcode Default (Dark) or Apple system colour, a darker or lighter
# shade of a family keeping its distance in lightness and its share of
# chroma (OKLCH). Warm greys become cool ones. Vendored originals are
# untouched.
PALETTE = {
    # Gruvbox's main colour: its replacement
    "#7daea3": "#41a1c0",  # blue
    "#ea6962": "#fc6a5d",  # red
    "#e3ae5a": "#d0bf69",  # yellow
    "#a9b665": "#78c88a",  # green (Apple's system green, softened)
    "#89b482": "#67b7a4",  # teal
    "#e78a4e": "#fd8f3f",  # orange
    "#d3869b": "#fc5fa3",  # pink
}
NEUTRALS = {
    "#45403d": "#3b4558",  # the folder body: slate with a little of the navy
    "#665c54": "#5b6678",
    "#ebdbb2": "#e6e8ee",
    "#796a45": "#6c7986",
    "#928374": "#8590a2",  # Gruvbox's grey: auxiliary files (indexes)
}
REPAINTED = ("gruvbox", "sandhill")

# Light theme (Huan, 2026-11-14): the palette is made for dark backgrounds.
# For the light theme each file/folder icon gets a variant whose
# colours reach LIGHT_CONTRAST against LIGHT_SURFACE (the light panes, with
# their hover and pale fills): a too-light colour is darkened in OKLCH,
# keeping its hue and as much chroma as fits in sRGB. Colours already clear
# enough, and the dark theme's icons, are unchanged.
LIGHT_SURFACE = "#efefef"
LIGHT_CONTRAST = 4.5
# A folder's emblem sits partly on its dark body (#3b4558): darkened to 4.5
# it would sink into the body, so emblems stop at 3:1, which still leaves
# about 3:1 against the body.
FOLDER_CONTRAST = 3.0


def _linear(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def _gamma(c):
    c = min(max(c, 0.0), 1.0)
    return 12.92 * c if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055


def _rgb(hex_color):
    h = hex_color.lstrip("#")
    if len(h) == 3:
        h = "".join(c * 2 for c in h)
    return [int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)]


def luminance(hex_color):
    r, g, b = (_linear(c) for c in _rgb(hex_color))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    hi, lo = sorted((luminance(a), luminance(b)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


def _to_oklch(hex_color):
    r, g, b = (_linear(c) for c in _rgb(hex_color))
    l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b) ** (1 / 3)
    m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b) ** (1 / 3)
    s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b) ** (1 / 3)
    L = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
    a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
    bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
    return L, (a * a + bb * bb) ** 0.5, __import__("math").atan2(bb, a)


def _from_oklch(L, C, H):
    import math
    a, b = C * math.cos(H), C * math.sin(H)
    l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3
    m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3
    s = (L - 0.0894841775 * a - 1.2914855480 * b) ** 3
    rgb = (4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
           -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
           -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s)
    if any(c < -1e-4 or c > 1 + 1e-4 for c in rgb):
        return None
    return "#" + "".join(f"{round(_gamma(c) * 255):02x}" for c in rgb)


def for_light(hex_color, target=LIGHT_CONTRAST):
    """`hex_color`, darkened just enough to reach `target` on the light theme."""
    if contrast(hex_color, LIGHT_SURFACE) >= target:
        return hex_color.lower()
    L, C, H = _to_oklch(hex_color)
    while L > 0:
        L -= 0.005
        chroma = C
        found = _from_oklch(L, chroma, H)
        while found is None and chroma > 0:
            chroma = max(chroma - 0.005, 0)
            found = _from_oklch(L, chroma, H)
        if found and contrast(found, LIGHT_SURFACE) >= target:
            return found
    return "#000000"


def repaint(svg, convert):
    """Passes every hex colour in `svg` through `convert`; whether any changed."""
    changed = False
    paint = re.compile(r"#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b")
    for node in svg.iter():
        for attr in ("fill", "stroke", "stop-color", "style"):
            value = node.get(attr)
            if not value:
                continue
            new = paint.sub(lambda m: convert(m.group(0)), value)
            if new.lower() != value.lower():
                node.set(attr, new)
                changed = True
    return changed


def light_variant(svg, target=LIGHT_CONTRAST):
    """A copy of a coloured icon for the light theme, or None if every
    colour already reads there."""
    light = copy.deepcopy(svg)
    return light if repaint(light, lambda c: for_light(c, target)) else None


def in_palette(hex_color):
    """A Gruvbox colour in Sandhill's palette (see PALETTE)."""
    color = hex_color.lower()
    if len(color) == 4:
        color = "#" + "".join(c * 2 for c in color[1:])
    if color in NEUTRALS:
        return NEUTRALS[color]
    L, C, H = _to_oklch(color)
    if C < 0.02:
        return color

    def turn(main):
        d = abs(_to_oklch(main)[2] - H)
        return min(d, 360 - d)

    main = min(PALETTE, key=turn)
    main_l, main_c, _ = _to_oklch(main)
    to_l, to_c, to_h = _to_oklch(PALETTE[main])
    lightness = max(0.0, min(1.0, to_l + (L - main_l)))
    chroma = to_c * C / main_c
    while chroma >= 0:
        found = _from_oklch(lightness, chroma, to_h)
        if found:
            return found
        chroma -= 0.005
    return color


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def normalize(path, ident, monochrome, logo, filled=False, recolor=None, strip_style=False,
              view_box=None):
    """Fit original geometry into 16px; UI masks follow the surrounding ink.

    Logos can use a small light backplate for dark artwork. Source maps may
    override individual paint colours without changing vendored originals,
    drop a source's <style> (e.g. prefers-color-scheme rules the app's
    SVG renderer cannot follow), and crop to the mark with their own
    viewBox when the original keeps empty margins. Prefix SVG ids so
    gradients remain local in the mockup.
    """
    source_text = path.read_text()
    root = ET.fromstring(source_text)
    if strip_style:
        for parent in list(root.iter()):
            for child in list(parent):
                if child.tag == f"{{{NS}}}style":
                    parent.remove(child)
    paints = {color.lower(): replacement for color, replacement in (recolor or {}).items()}
    view = view_box or root.get("viewBox")
    if view:
        x, y, width, height = map(float, re.split(r"[\s,]+", view.strip()))
    else:
        x = y = 0
        width = float(root.get("width", "16").removesuffix("px"))
        height = float(root.get("height", "16").removesuffix("px"))
    inset = 2 if logo else 0
    scale = (16 - inset * 2) / max(width, height)
    tx = (16 - width * scale) / 2 - x * scale
    ty = (16 - height * scale) / 2 - y * scale
    prefix = ident.replace(".", "-") + "-"
    ids = {node.get("id") for node in root.iter() if node.get("id")}
    for node in root.iter():
        for key, value in list(node.attrib.items()):
            if key == "id":
                node.set(key, prefix + value)
            else:
                for old in ids:
                    value = value.replace(f"url(#{old})", f"url(#{prefix}{old})")
                    if key.endswith("href") and value == "#" + old:
                        value = "#" + prefix + old
                node.set(key, value)
        if paints:
            for attr in ("fill", "stroke"):
                value = node.get(attr)
                if value is not None:
                    node.set(attr, paints.get(value.strip().lower(), value))
            if "style" in node.attrib:
                node.set("style", re.sub(
                    r"((?:^|;)\s*(?:fill|stroke)\s*:\s*)([^;]+)",
                    lambda match: match[1] + paints.get(match[2].strip().lower(), match[2]),
                    node.attrib["style"], flags=re.I,
                ))
        if monochrome:
            for attr in ("fill", "stroke"):
                value = node.get(attr)
                if value is not None and value not in ("none", "transparent"):
                    node.set(attr, "currentColor")
            # Upstream action icons normally use presentation attributes;
            # handle inline paints too without discarding opacity/geometry.
            if "style" in node.attrib:
                node.set("style", re.sub(
                    r"(fill|stroke)\s*:\s*(?!none\b|transparent\b)[^;]+",
                    r"\1:currentColor", node.attrib["style"],
                ))
    if monochrome and filled:
        root.set("fill", "currentColor")
    attrs = {k: v for k, v in root.attrib.items()
             if k not in ("viewBox", "width", "height", "version", "id")}
    if monochrome:
        attrs.setdefault("fill", "currentColor")
    group = ET.Element(f"{{{NS}}}g", attrs)
    group.set("transform", f"matrix({scale:g} 0 0 {scale:g} {tx:g} {ty:g})")
    group.extend(list(root))
    output = ET.Element(f"{{{NS}}}svg", {"viewBox": "0 0 16 16"})
    for comment in re.findall(r"<!--(.*?)-->", source_text, re.S):
        output.append(ET.Comment(comment))
    if logo:
        ET.SubElement(output, f"{{{NS}}}rect", {
            "x": "1", "y": "1", "width": "14", "height": "14", "rx": "4",
            "fill": "#fff", "stroke": "#808080", "stroke-opacity": ".25",
            "stroke-width": ".6",
        })
    output.append(group)
    return output


def body(svg):
    return "".join(ET.tostring(node, encoding="unicode") for node in svg)


def preview(reg):
    groups = {}
    for ident, item in reg["icons"].items():
        group = item.get("group", item["kind"])
        groups.setdefault(group, []).append((ident, item))
    order = ["window", "edit", "files", "run", "resources", "plots", "git",
             "agent", "arrows", "file", "folder", "agent_chips", "language"]
    labels = {"window": "Window & toolbar", "edit": "Edit & document",
              "files": "Files & navigation", "run": "Running code",
              "resources": "Resources", "plots": "Plots & help", "git": "Git",
              "agent": "Agent controls", "arrows": "Arrows", "file": "File types",
              "folder": "Folders", "agent_chips": "Integrations",
              "language": "Languages & runtimes"}
    sections = []
    light_sections = []
    for group in order:
        items = groups.get("agent" if group == "agent_chips" else group, [])
        if group in ("agent", "agent_chips"):
            kind = "system" if group == "agent" else "agent"
            items = [(ident, item) for ident, item in items if item["kind"] == kind]
        cells = []
        for ident, item in items:
            svg = (HERE / item["file"]).read_text()
            title = html.escape(item["label"] + " · " + item["source"]["path"], quote=True)
            cells.append(f'<figure title="{title}">{svg}<figcaption>{ident}</figcaption></figure>')
        sections.append(f'<section><h2>{labels[group]}</h2><div class="grid">{"".join(cells)}</div></section>')
        if group in ("file", "folder"):
            light_cells = []
            for ident, item in items:
                svg = (HERE / item.get("light", item["file"])).read_text()
                title = html.escape(item["label"] + " · light theme", quote=True)
                light_cells.append(f'<figure title="{title}">{svg}<figcaption>{ident}</figcaption></figure>')
            light_sections.append((len(sections) - 1,
                f'<section><h2>{labels[group]}</h2><div class="grid">{"".join(light_cells)}</div></section>'))
    run = (HERE / "system/run.svg").read_text()
    states = "".join(f'<button class="st {cls}" {attrs}>{run}<span>{label}</span></button>'
                     for cls, attrs, label in [("", "", "Rest"), ("hover", "", "Hover"),
                                               ("sel", 'aria-pressed="true"', "Selected"),
                                               ("", "disabled", "Disabled")])
    sections.append(f'<section><h2>States</h2><div class="states">{states}</div></section>')
    content = "".join(sections)
    # The light column shows the light theme's file and folder icons.
    light = list(sections)
    for index, section in light_sections:
        light[index] = section
    light_content = "".join(light)
    return f'''<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sandhill icons · Gruvbox Material + Lucide + Developer Icons</title><style>
body {{ margin:0; font:12px/1.4 -apple-system,system-ui,sans-serif; }}
.cols {{ display:grid; grid-template-columns:1fr 1fr; }}
.theme {{ padding:18px 22px; background:#f3f3f4; color:#24262a; }}
.theme.dark {{ background:#24262a; color:#edf0f3; }}
h1 {{ font-size:15px; margin:0 0 4px; }}
.sub,figcaption {{ opacity:.7; }}
h2 {{ font-size:11px; text-transform:uppercase; margin:14px 0 6px; }}
.grid {{ display:grid; grid-template-columns:repeat(auto-fill,minmax(86px,1fr)); gap:2px; }}
figure {{ margin:0; padding:6px 2px; display:flex; flex-direction:column;
 align-items:center; gap:4px; border-radius:6px; }}
figure svg {{ width:16px; height:16px; }}
figcaption {{ font-size:10px; text-align:center; overflow-wrap:anywhere; }}
.states {{ display:flex; gap:4px; flex-wrap:wrap; }}
.st {{ display:flex; align-items:center; gap:6px; font:inherit; color:inherit;
 background:none; border:0; border-radius:6px; padding:5px 8px; }}
.st svg {{ width:16px; height:16px; }}
.st.hover {{ background:color-mix(in srgb,currentColor 9%,transparent); }}
.st.sel {{ background:color-mix(in srgb,#4981c1 16%,transparent); }}
.st:disabled {{ opacity:.38; }}
@media(max-width:900px) {{ .cols {{ grid-template-columns:1fr; }} }}
</style></head><body><div class="cols">
<div class="theme"><h1>Sandhill icons</h1><p class="sub">Gruvbox Material (repainted) + Lucide + Developer Icons · 16px · Light (file and folder colours darkened to {LIGHT_CONTRAST}:1 on {LIGHT_SURFACE})</p>{light_content}</div>
<div class="theme dark"><h1>Sandhill icons</h1><p class="sub">Same assets · Dark</p>{content}</div>
</div></body></html>'''


def main():
    catalog = json.loads((HERE / "catalog.json").read_text())
    sources = {}
    for name in ("system-sources.json", "content-sources.json"):
        entries = json.loads((HERE / name).read_text())
        if sources.keys() & entries.keys():
            raise ValueError(f"Duplicate icon source in {name}")
        sources.update(entries)
    if sources.keys() != catalog["icons"].keys():
        raise ValueError(f"Icon source coverage mismatch: {sources.keys() ^ catalog['icons'].keys()}")
    reg = copy.deepcopy(catalog)
    reg["source"] = "docs/design/ui/icons/build.py"
    reg["libraries"] = json.loads((HERE / "vendor/libraries.json").read_text())
    artwork = {}
    for ident, item in reg["icons"].items():
        source = sources[ident]
        path = HERE / "vendor" / source["library"] / source["path"]
        svg = normalize(
            path, ident, item["kind"] == "system",
            source.get("backplate", source["library"] == "developer"),
            source.get("filled", False),
            source.get("recolor"),
            source.get("strip_style", False),
            source.get("viewBox"),
        )
        if item["kind"] in ("file", "folder") and source["library"] in REPAINTED:
            repaint(svg, in_palette)
        if item["kind"] == "system":
            svg.set("aria-hidden", "true")
        else:
            # SVG images do not inherit the app's text colour. Give generic
            # Lucide content symbols neutral ink; controls still inherit it.
            if source["library"] == "lucide":
                for node in svg.iter():
                    for attr in ("fill", "stroke"):
                        if node.get(attr) == "currentColor":
                            node.set(attr, "#808080")
            svg.set("role", "img")
            svg.set("aria-label", item["label"])
            ET.SubElement(svg, f"{{{NS}}}title").text = item["label"]
        item["source"] = source
        write(HERE / item["file"], ET.tostring(svg, encoding="unicode") + "\n")
        artwork[ident] = body(svg)
        light_path = HERE / item["file"].replace(".svg", ".light.svg")
        light = None
        if item["kind"] in ("file", "folder"):
            target = FOLDER_CONTRAST if item["kind"] == "folder" else LIGHT_CONTRAST
            light = light_variant(svg, target)
        if light is not None:
            item["light"] = item["file"].replace(".svg", ".light.svg")
            write(light_path, ET.tostring(light, encoding="unicode") + "\n")
        elif light_path.exists():
            light_path.unlink()
    write(HERE / "registry.json", json.dumps(reg, indent=1) + "\n")
    rows = ["# Sandhill icon sources", "", "Generated by `build.py` from the two source maps.",
            "Original files and licenses are in `vendor/`; pinned revisions are in `vendor/libraries.json`.",
            "", "| Sandhill ID | Legacy mockup IDs | Source | Substitution note |",
            "|---|---|---|---|"]
    for ident, item in reg["icons"].items():
        source = item["source"]
        rows.append(f"| `{ident}` | {', '.join(item.get('v31', []))} | `{source['library']}/{source['path']}` | {source.get('note', '')} |")
    rows += ["", "File/folder association rules are preserved in `catalog.json`.",
             "Compression suffixes are stripped before extension matching, so `.fastq.gz` remains FASTQ."]
    write(HERE / "MAPPING.md", "\n".join(rows) + "\n")
    js = {"bio": {ident[5:]: artwork[ident] for ident in artwork if ident.startswith("file.")},
          "folders": {ident[7:]: artwork[ident] for ident in artwork if ident.startswith("folder.")},
          "resolve": reg["resolve"],
          "lang": {key: artwork["lang." + name] for key, name in
                   [("r", "r"), ("py", "python"), ("nf", "nextflow"), ("sh", "shell")]}}
    block = "/* ICONS32 (generated by icons/build.py; edit source maps there) */\nconst ICONS32 = " + json.dumps(js, separators=(",", ":")) + ";\n/* /ICONS32 */"
    text = MOCKUP.read_text()
    text, replaced = re.subn(r"/\* ICONS32 .*?/\* /ICONS32 \*/", lambda _: block, text, flags=re.S)
    if replaced != 1:
        raise ValueError("Missing or duplicate generated mockup block")
    for ident, item in reg["icons"].items():
        if item["kind"] not in ("system", "agent"):
            continue
        ids = item.get("v31") or (["i-" + ident.replace(".", "-")] if item["kind"] == "system" else [])
        for old in ids:
            symbol = f'<symbol id="{old}" viewBox="0 0 16 16">{artwork[ident]}</symbol>'
            pattern = rf'<symbol id="{re.escape(old)}"[^>]*>.*?</symbol>'
            text, count = re.subn(pattern, lambda _: symbol, text, flags=re.S)
            if count == 0:
                if item.get("v31"):
                    raise ValueError(f"Missing legacy symbol {old}")
                # New controls have no legacy slot. Generate their sprite
                # entry beside the existing controls, rather than hand-copying SVG.
                text = text.replace("</svg>", f"  {symbol}\n</svg>", 1)
    if text != MOCKUP.read_text():
        MOCKUP.write_text(text)
    write(HERE / "preview.html", preview(reg))
    print("icons:", dict(Counter(item["kind"] for item in reg["icons"].values())),
          "| sources:", dict(Counter(source["library"] for source in sources.values())))


if __name__ == "__main__":
    main()
