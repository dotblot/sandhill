# Sandhill icons V32.2

Every icon Sandhill draws, one SVG per icon, with a registry the app can read.
Open [`preview.html`](preview.html) to see them all on the light and dark panes
(`../icon-sheet.png` is a screenshot of it).

```sh
python3 docs/design/ui/icons/build.py   # rewrites everything here and the mockup's icon block
```

Change icons in `build.py`, not in the SVGs: it writes the SVGs, `registry.json`,
`MAPPING.md`, `preview.html` and the `ICONS32` block in `../sandhill-layout.html`.
The line icons are read from the mockup's symbol sprite, so a line icon is
edited there and then the script is run.

| Folder | What | Colour |
|---|---|---|
| `system/` | 91 line icons: window, edit, files, running code, resources, plots, Git, agent controls, arrows | `currentColor` |
| `bio/` | 35 file icons: one page outline, a symbol per type | one colour per family |
| `folders/` | 10 folder icons | one colour per kind; the emblem uses `var(--tv-cut)` (the pane background) |
| `agents/` | 5 agent chips | fixed |
| `lang/` | 4 language and runtime chips | fixed |

## Drawing rules
- A 16 × 16 grid. Line icons: 1.4 stroke, round caps and joins, no box behind
  them, and `currentColor`, so they follow the theme and the control's state.
- File icons: the page is 10 × 13 with a folded corner, stroked at 1.1 and
  filled with its colour at 13%; the symbol inside is stroked at 1.25. The
  colour says the family (sequences, genomics, data, tables, workflows…), the
  symbol says the type. Every family colour has at least 3:1 contrast on both
  the light pane (`#eeebe7`) and the dark one (`#221f1d`), so no icon is
  inverted or recoloured for dark mode.
- Chips: a 14 × 14 rounded square (radius 4), with a white mark drawn as
  strokes, so they look the same whatever fonts are installed.

## Using them
- `registry.json` → `icons` maps each id (`run`, `file.fastq`, `folder.data`,
  `agent.omp`…) to its file, label, colour and the V31 ids it replaces.
- `registry.json` → `resolve` picks a file's icon: names first (`Snakefile`,
  `CLAUDE.md`), then name prefixes (`README*`), then the extension after
  compression suffixes are stripped (`reads.fastq.gz` → `file.fastq`), then
  `file.file`. Folders match on their name, else `folder.other`. An icon only
  changes how a file or folder looks, never how Sandhill treats it.
- States: hover and selected change the control's background and `color`, and
  disabled is `opacity: .38`. File and chip icons don't change colour.
- Accessibility: an icon next to a text label is decorative
  (`aria-hidden="true"`, as the SVGs ship). An icon-only button needs an
  `aria-label` on the button (the registry's `label` is a good default). The
  chips carry a `<title>` and `role="img"` because they stand alone in tabs.
- To use a vendor's official mark later, add it under `agents/`, point the
  agent's `file` at it and set `brand` in the registry. Callers keep using the
  id.

## Licences
Every file here is drawn for Sandhill and belongs to the project. Nothing is
copied from an icon library, a font or an AI image.

| Item | Source | Licence / status |
|---|---|---|
| Line icons (`system/`) | Drawn for Sandhill, drafts 1–33, in the style of SF Symbols and Lucide | Sandhill's own. No Apple SF Symbols and no Lucide files are included |
| File and folder icons | Drawn for Sandhill | Sandhill's own |
| Claude Code, Codex, Antigravity chips | Sandhill monograms "CC", "Cx", "Ag" | Sandhill's own. The official marks are trademarks of Anthropic, OpenAI and Google; they need written permission for a product that is sold |
| omp chip (π and cursor) | Drawn for Sandhill | Sandhill's own. It is not Oh My Pi's logo and must not be called one |
| R, Python, Nextflow, Shell chips | Sandhill letter chips | Sandhill's own. The R logo is CC BY-SA 4.0 (credit, and changes must be shared alike); the Python logo is a PSF trademark under its trademark policy; the Nextflow logo is Seqera's. None of them is used |
| Product names in text | — | Fine to use to say what Sandhill works with |
