# Sandhill icons

Sandhill's 167 shared interface, file, folder, language and integration icons
use [Gruvbox Material Icons](https://github.com/navernoedenis/gruvbox-material-icons),
[Lucide](https://github.com/lucide-icons/lucide) and
[Developer Icons](https://github.com/xandemon/developer-icons), selected by
Huan on 2026-11-06, and 17 file icons drawn for Sandhill in Gruvbox's
document style (Huan, 2026-11-14). Posit Assistant and Antigravity use Huan's supplied brand
references, and Oh My Pi and pi their official MIT-licensed marks, instead of
generic symbols. This replaces Atom Material completely.
Sandhill's crane and macOS app icon remain separate in
[`../brand/`](../brand/README.md). Fonts are unchanged.

Open [`preview.html`](preview.html) for every icon in light and dark themes;
[`../icon-sheet.png`](../icon-sheet.png) is a browser capture of that page.

| Output folder | Purpose | Count |
|---|---|---|
| `system/` | Window, toolbar, editor, run, resources, Git and agent controls | 105 |
| `bio/` | File types, including bioinformatics formats | 40 |
| `folders/` | Project, data, results, figures, notebooks and other folders | 10 |
| `agents/` | Agent integrations | 8 |
| `lang/` | R, Python, Nextflow and shell runtimes | 4 |

## Source of truth

```sh
python3 docs/design/ui/icons/build.py
```

The generator is offline and uses only Python's standard library. No npm
package, icon font or network connection is needed to rebuild the assets.

- `catalog.json`: stable Sandhill IDs, labels, categories, legacy mockup IDs,
  and file/folder association rules.
- `system-sources.json` and `content-sources.json`: the selected upstream SVG
  for each ID, including notes for generic substitutes. `backplate` controls
  the light logo backing; `recolor` maps source paint colours to replacements
  in fill/stroke attributes and inline styles without changing vendor originals;
  `viewBox` crops an original with empty margins to its mark.
- `vendor/`: 166 selected SVGs: pinned library originals, Sandhill's own
  drawings (`vendor/sandhill/`), the supplied Antigravity and Synthetic
  Sciences SVGs, a trace of the supplied Posit Assistant mark, and the
  official Oh My Pi and pi marks.
- `vendor/libraries.json`: repository URLs, pinned revisions and provenance.
- Generated output: the SVG folders (file and folder icons with a
  `*.light.svg` beside each), `registry.json`, `MAPPING.md`, `preview.html`,
  and the icon sprite/generated block in the UI mockup.

Change the catalog or source maps, not generated SVGs. When adding an upstream
asset, retain its original path below `vendor/<library>/`, preserve its
license comments, and update the pinned revision if necessary. The generator
requires an explicit source for every catalog ID; it never falls back to the
retired hand-drawn icon set. Raw vendor assets are excluded from the app's
embedded asset set; only normalized output SVGs and the registry are embedded.

## Rendering

- Original geometry is proportionally fitted into a 16px viewBox, including
  source files with negative viewBox origins.
- Lucide supplies all 105 interface icons. They use `currentColor` and follow
  the control's normal, hovered, selected and disabled text colours. The
  selected favourite star fills the same upstream star geometry via the
  source map's `filled` flag. No new interaction or animation is introduced.
- Gruvbox supplies 33 file and folder icons and Sandhill's drawings the other
  17. Their shapes are kept and their colours replaced (Huan, 2026-11-14):
  Gruvbox's warm palette did not sit with Sandhill's cool glass, navy and
  Xcode code colours. `build.py` (`PALETTE`, `in_palette`) moves each Gruvbox
  hue family to the matching Xcode Default (Dark) or Apple system colour,
  blue `#41a1c0`, red `#fc6a5d`, yellow `#d0bf69`, green `#78c88a`, teal
  `#67b7a4`, orange `#fd8f3f`, pink `#fc5fa3`; a family's darker shade keeps
  its distance in lightness and its share of chroma, and warm greys become
  cool (the folder body is slate `#3b4558`). Vendored originals are
  untouched. The palette is made for dark panes, so each icon also gets a
  `*.light.svg` for the light theme: every colour under 4.5:1 on `#efefef`
  is darkened in OKLCH, hue kept and chroma reduced only to stay in sRGB,
  until it reaches 4.5:1. Folder emblems stop at 3:1, because they sit
  partly on the dark folder body and would sink into it. The app picks the
  light versions while the light theme is on (`look::picture`).
- Sandhill's drawings use Gruvbox's filled document with a folded corner,
  its palette, and a glyph cut out of the document with a mask (strokes of
  1.6 on its 24px grid): FASTA a helix, FASTQ a helix over quality bars,
  BAM/SAM/CRAM a pileup of reads, VCF a variant pinned on a sequence,
  GFF/GTF a gene model, BED bounded intervals, h5ad an annotated matrix, mtx
  a sparse matrix, HDF5/Loom a tree of groups, Parquet columns, RDS/RData a
  box, Nextflow, WDL and CWL workflows that join, chain and fan out, index
  files (BAI, FAI, TBI…) a list of entries in grey, signal tracks (bigWig,
  wiggle) a coverage curve, and phylogenetic trees a rooted tree. One icon
  covers a family of extensions (`catalog.json`): PAF, PSL and BLAST use the
  alignment icon, gVCF and MAF the variant one, peaks and BEDPE the interval
  one, h5mu AnnData's, cool/mcool HDF5's, PLINK's `.bim`, `.fam`, `.pgen`,
  `.pvar` and `.psam` the variant one.
- Two rules look beyond a file's own name. A folder whose extension is in
  `resolve.folderExtensions` (`.zarr`) is one data set and shows that
  format's file icon instead of a folder. PLINK's binary genotypes are
  `.bed` files, as BED intervals are: a `.bed` with a `.bim` or `.fam` of
  the same name beside it shows the variant icon (`look::file_icon_beside`,
  in the Files pane, where the neighbours are known).
- Developer logos keep a small white rounded backplate by default so dark
  artwork stays visible on dark panes. Claude and Codex are transparent:
  Claude retains its source orange, and Codex uses blue `#2F80ED` instead of
  the upstream dark green. Runtime logos retain their source colours.
- Three generic content icons use Lucide with neutral grey ink, because
  standalone image assets cannot inherit the surrounding control's text colour.
- SVG definition IDs are namespaced to avoid gradient collisions in the mockup.
- Accessible labels and titles are retained for standalone content icons.

## Selection and substitutions

All existing icon IDs and file/folder matching rules remain unchanged.
Compressed suffixes are stripped before extension matching: `reads.fastq.gz`
still uses the FASTQ icon. Changing a folder icon does not change its behavior.

Gruvbox includes dedicated Jupyter, Quarto, Snakemake, PDF, SVG and agent
artwork (the last for CLAUDE.md and AGENTS.md). Formats Gruvbox has no
artwork for are drawn for Sandhill; only unknown files keep the generic
document. A project's own icon is the project folder unless one is chosen.
Developer Icons supplies the R, Python, Bash, Claude and OpenAI integration
and runtime logos; OpenAI's logo identifies Codex. Agent marks are drawn
without a background (Huan). Codex's OpenAI logo and OpenScience's Synthetic
Sciences mark are one colour, drawn in the text colour so they read on light
and dark. Posit Assistant uses Posit's blue and orange mark from posit.co,
without the wordmark. Antigravity uses the supplied full-colour SVG
unchanged apart from normalization. Oh My Pi uses omp.sh's gradient π without
its dark tile; pi uses pi.dev's mark. Both are cropped to the mark.
Lucide supplies generic symbols for unknown agents and the Nextflow runtime.
Every choice is listed in [`MAPPING.md`](MAPPING.md).

## Licenses and attribution

Gruvbox and Developer Icons are MIT-licensed. Lucide uses ISC and includes
MIT terms for Feather-derived icons. Their artwork retains those licenses,
not Sandhill's GPL license; Sandhill's own drawings are GPL-3.0-or-later.
Original SVG license comments and upstream
acknowledgements are kept:

- [`vendor/gruvbox/LICENSE`](vendor/gruvbox/LICENSE)
- [`vendor/lucide/LICENSE`](vendor/lucide/LICENSE)
- [`vendor/developer/LICENSE`](vendor/developer/LICENSE)
- [`vendor/brands/oh-my-pi.LICENSE`](vendor/brands/oh-my-pi.LICENSE)
- [`vendor/brands/pi.LICENSE`](vendor/brands/pi.LICENSE)
- [`vendor/NOTICE.txt`](vendor/NOTICE.txt)

The Mac packaging workflow includes these notices in
`Contents/Resources/licenses/icons/`. Logos may be trademarks; the upstream
licenses do not grant trademark rights or imply the vendors endorse Sandhill.
The crane and app icon are Sandhill's own and are not replaced.

The Antigravity, Synthetic Sciences and Posit marks are not covered by the library licenses.
No redistribution license was supplied for them; record permission or applicable
brand-use terms before a public release. Attribution alone is not permission.
