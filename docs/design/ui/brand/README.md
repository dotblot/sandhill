# Sandhill crane mark V1.1 (draft)

A Wisconsin sandhill crane flying right: two raised wings, a sage inner
feather, a long neck and pointed beak, a red crown and two trailing legs.
[`brand-sheet.png`](brand-sheet.png) shows every version, and
[`before-after.png`](before-after.png) compares it with the app's current icon
and the V1.1 concept board.

**Status: a draft for Huan to approve. It is not in the app.** The app's icon
is still Huan's painting (`app/src-tauri/icons/source.png`, two standing
cranes at sunset). The repository has no vector of the flying-crane logo, so
these paths were drawn by hand, using the V1.1 concept board's pose as a guide.
Nothing is traced from or cut out of that AI image. If the original logo exists
as a vector, it should replace this geometry, keeping the V1.1 changes below.

```sh
python3 docs/design/ui/brand/build.py   # SVGs
node docs/design/ui/brand/render.mjs    # PNGs (Playwright)
```

## Geometry
- Seven shapes on a 1024 canvas: back wing, front wing, body with neck and head,
  two legs, the feather and the crown. The wings, body and legs share one
  path, so they fill without seams. The small mark has one leg instead of two.
- **Every version uses the same paths**; light and dark differ only in colour.
- The V1.1 changes, as the brief asks:
  - Crown: a low cap that follows the forehead, with a thin gap above the head,
    and no bump behind. The small mark's crown is a little larger, so it still
    reads at 16px.
  - Wings: the back wing's upper edge is raised a little, to balance the front
    wing; the tips are single smooth curves; there is no feather texture.
  - Feather: a narrow blade tucked between the wings, with clear space on both
    sides. It stays on the wing, not as a separate leaf.
  - Wing root: the wings run into the body under it, so there is no crossing of
    points where they meet.
  - Legs: thicker, with rounded ends (no hairline tips). The rear leg joins the
    tail. The small mark keeps one shorter, thicker leg.

## Colours
Navy `#142844`, cream `#F8F1E4`, sage `#6FA38E`, crane red `#E64B3C` (the
brief's colours; the concept board's are within a few steps of them).

## Files
| File | Use |
|---|---|
| `crane/sandhill-crane-light.svg` | Navy crane on cream (website, About, splash) |
| `crane/sandhill-crane-dark.svg` | Cream crane on navy |
| `crane/sandhill-crane-on-light.svg`, `-on-dark.svg` | The same, transparent background |
| `crane/sandhill-crane-mono-navy.svg`, `-mono-cream.svg` | One colour; the crown and feather keep their own shapes |
| `crane/sandhill-crane-template.svg` | macOS menu bar template: black on transparent, 18pt, the small drawing |
| `small/sandhill-mark-{64,32,24}.svg` | Small mark: one leg, larger crown |
| `small/sandhill-mark-16.svg`, `-16-dark.svg` | 16px: no feather, crown kept |
| `app-icon/app-icon-dark.svg` | macOS app icon: an 824px tile with radius 185 on a 1024 canvas (Apple's grid), navy gradient, cream crane |
| `app-icon/app-icon-*-small.svg` | The app icon for 16 and 32px: small drawing, larger |
| `app-icon/app-icon-light.svg` | A cream alternative |
| `png/`, `app-icon/png/` | PNG renders; the app icon at 16–1024px |

## Putting it in the app (not done)
Once approved, and once the app session is free to take it:
1. Put `app-icon/png/app-icon-1024.png` as `app/src-tauri/icons/source.png`, and
   change `app/scripts/icon.mjs` so it doesn't clip or crop again: this artwork
   already has the tile, and its 16 and 32px sizes come from
   `app-icon-dark-small.svg`.
2. Run `npm run icon` in `app/` to regenerate `icon.icns`, `icon.png` and the PNGs
   that `tauri.conf.json` lists. The bundle id, app name and signing don't change.
3. The start screen shows `icon.png` and the title bar shows `32x32.png`, so they
   follow without code changes.
