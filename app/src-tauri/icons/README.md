# App icon

The Sandhill crane (V1.1), from `docs/design/ui/brand`. `source.svg` is the
app icon: an 824 px rounded tile on Apple's 1024 px grid, navy, with the cream
crane. `source-small.svg` is the version drawn for 16 and 32 px (one leg, a
larger crown, no feather). Both are written by
`docs/design/ui/brand/build.py`; change them there. The other files are made
from them:

```sh
python3 ../docs/design/ui/brand/build.py   # from app/, if the crane changed
npm run icon
```

`scripts/icon.mjs` adds the soft shadow macOS icons have to the large sizes
and renders them with `tauri icon`. The 16 and 32 px sizes in `icon.icns`
(and `32x32.png`) come from `source-small.svg`.

The app's start screen shows `icon.png`. Until October 2026 the icon was Huan's
painting of two cranes at sunset; it is in the git history.
