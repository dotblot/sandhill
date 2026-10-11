# App icon

`source.png` is the icon artwork (two large sandhill cranes in front of the
sun, chosen in October 2026 because they read better at small sizes than the
earlier sunset scene), 1024 px, with the light canvas around the rounded
square made transparent and its shadow kept as translucent black. The other files
are generated from it:

```sh
cd app
npx tauri icon src-tauri/icons/source.png -o /tmp/icons
```

then copy `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.png` and
`icon.icns` here; those are the ones `tauri.conf.json` uses.
