# App icon

`source.png` is the icon artwork, as Huan made it (October 2026): two
sandhill cranes in front of an orange sunset, a square painting that fills
its canvas. It is the image from `sandhill-final.svg`, unchanged. The other
files are generated from it:

```sh
cd app
npm run icon
```

`scripts/icon.mjs` clips the painting to a rounded square in Apple's icon
grid (824 of 1024 px) with a soft shadow, so it sits at the same size as
other apps in the Dock. At 16 and 32 px the whole scene blurs, so those
sizes in `icon.icns` (and `32x32.png`) come from a closer crop on the cranes'
heads and necks. The crop is set in the script, in the painting's pixels;
check it if the artwork changes.

The app's start screen shows `icon.png` and its title bar `32x32.png`.
