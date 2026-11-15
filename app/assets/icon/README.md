# App icon

The Sandhill crane (V1.1), from `docs/design/ui/brand`. `source.svg` is the
app icon: an 824 px rounded tile on Apple's 1024 px grid, navy, with the white
crane. `source-small.svg` is the version drawn for 16 and 32 px (one leg, a
larger crown, no feather). Both are written by
`docs/design/ui/brand/build.py`; change them there, then make the icon:

```sh
python3 docs/design/ui/brand/build.py   # if the crane changed
node app/assets/icon/make.mjs
```

`make.mjs` adds the soft shadow macOS icons have to the large sizes and
renders them with Tauri's icon tool (fetched by npx). The 16 and 32 px sizes
in `icon.icns` come from `source-small.svg`. It also writes
`../brand/icon.png`, which the Start screen shows, and compiles
`Sandhill.icon` (also from `build.py`) into `Assets.car` with Xcode's
`actool` (Xcode 26 or later). The bundle (`.github/workflows/mac-app.yml`)
puts both in Sandhill.app: `CFBundleIconName` names the `Assets.car` icon,
which macOS 26 and later draw in light, dark, tinted and clear, and older
macOS uses `icon.icns`.

Until October 2026 the icon was Huan's painting of two cranes at sunset; it is
in the git history.
