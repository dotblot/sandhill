# App icon

`source.svg` is the icon artwork, as Huan drew it (October 2026): two
sandhill cranes in front of an orange sun, on a transparent background. The
other files are generated from it:

```sh
cd app
npm run icon
```

`scripts/icon.mjs` fits the artwork into Apple's icon grid (824 of 1024 px),
so it sits at the same size as other apps in the Dock. At 16 and 32 px the
whole scene blurs, so those sizes in `icon.icns` (and `32x32.png`) come from
a closer crop on the cranes and the sun, without the reeds and ripples. The
crop is set in the script, in the artwork's coordinates; check it if the
artwork changes.
