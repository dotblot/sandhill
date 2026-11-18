# Fonts and text sizes

Status: approved by Huan and built (2026-11-18).

## What Huan asked for
- Code uses the Mac's own code font by default (SF Mono).
- In Welcome → General, people can pick any installed font for code, as
  long as it is monospaced (every letter the same width).
- The interface always uses the Mac's interface font (SF Pro); it is not
  a choice.
- People can change the size of the code text and of the interface text.

## What is true today
- The interface already uses the system font (`.SystemUIFont`).
- Code uses Maple Mono NF, which Sandhill bundles (4.9 MB in the app),
  through `look::MONO` (44 places in 26 files) and the Kit theme's
  `mono_font_family`.
- Sizes are fixed numbers in `app/src/look.rs`: interface 15, code 15,
  small 13, titles 16 and 24, tabs 14, tiny 11. `SMALL` alone is used in
  about 230 places.
- SF Mono can't be asked for by its name, but the Mac offers it under a
  hidden name, `.AppleSystemUIFontMonospaced`, with all weights and
  italics: checked on this Mac, 12 faces. That works the same way the
  interface font already does, so no font file is needed.

## The change
1. **One font setting, one place.** `look::MONO` stops being a fixed name.
   A small app-wide setting holds the code font, the code size and the
   interface size, saved with the other preferences on this Mac. Every
   place that now says `look::MONO`, `look::CODE` or a text size asks it
   instead.
2. **Default code font is SF Mono**, via the hidden name above.
3. **Welcome → General gets a "Fonts" group:**
   - Code font: a dropdown with "System (SF Mono)" first, then every
     installed monospaced font in alphabetical order. Sandhill finds them
     by measuring a narrow and a wide letter in each font, so it doesn't
     depend on a font calling itself monospaced.
   - Code size: a number with − and + (10 to 24, default 15).
   - Interface size: a number with − and + (12 to 18, default 15).
   - "Restore defaults".
   Changes show at once in every open window, with no restart.
4. **What follows the code font and size:** the editor, notebook cells,
   the Console, the terminal, outputs and tables, Git diffs, code in
   Help and AI replies, the Viewer's plain text. Exported HTML keeps
   `ui-monospace`, which is SF Mono in a browser.
5. **What follows the interface size:** all interface text keeps its
   steps (small is 2 below, titles above), moving up or down together.
   Row, tab and button heights grow with it, so larger text never gets cut
   off. Gaps, corners and icons stay the same size.
6. **A font that disappears** (uninstalled since it was chosen) falls back
   to SF Mono, and General says so beside the dropdown.
7. **Records:** `docs/decisions/look.md` gets the new font rule with its
   date; `docs/design/ui/spec.md` and the UI rules in `CLAUDE.md` say sizes
   come from the setting, not fixed numbers.

## Open questions for Huan
- **Maple Mono:** stop bundling it (the app gets 4.9 MB smaller and the
  question to the font author goes away; anyone who wants it installs it
  and picks it), or keep it bundled as one of the choices?
  Note: Maple Mono NF carries the extra icons some terminal prompts draw
  (Nerd Font icons). Without it, those prompts show empty boxes unless the
  person has a Nerd Font installed and picks it.
- **Defaults:** keep 15 for both sizes, as now?

## Checks
- A test for the font list (monospaced fonts in, others out) and for the
  fallback when a chosen font is missing.
- Screenshots of a notebook, the Console, the terminal and Welcome at the
  smallest and largest sizes, light and dark.
- The usual `app/` checks.
