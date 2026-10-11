# App icon

`source.png` is the icon artwork (two sandhill cranes at sunset), 1024 px,
with the canvas around the rounded square made transparent. The other files
are generated from it:

```sh
cd app
npx tauri icon src-tauri/icons/source.png -o /tmp/icons
```

then copy `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.png` and
`icon.icns` here; those are the ones `tauri.conf.json` uses.
