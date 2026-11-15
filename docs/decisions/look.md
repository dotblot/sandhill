# Look

Settled decisions about how Sandhill looks. Moved here unchanged from
`CLAUDE.md` on 2026-11-15; where an older plan differs, this wins.

## The Mac app
- Mac app: GPUI (Rust, in `app/`; `gpui/` until 2026-11-15), Sandhill.app
  with bundle id `dev.sandhill.ide`; its program is `sandhill`. It replaced
  the Tauri 2 app (TypeScript UI, then also in `app/`, decided 2026-11-01,
  retired 2026-11-02; it is in the git history). It keeps
  the Tauri app's sizes, with every font size 2px larger. No Electron.

## Glass, colours and tabs
- Look (Huan, 2026-11-05): the window is frosted glass (the system blurs
  what is behind it) with the panes floating on it, as in Xcode 27. The
  editor is solid; the side panels and the Console, Terminal and Jobs under
  the editor are translucent. Pane tab bars and tool rows float over their
  scrolling content at 82% opacity, so it shows faintly beneath (Huan,
  2026-11-11); GPUI has no per-element blur. Terminals, Plots, Viewer and
  code editors keep stacked headers (Huan kept this on 2026-11-13: Kit's
  Editor resets its own padding every frame, so a floating header would
  cover the first line). Notebook cells hide line numbers;
  scripts and Quarto Source view keep them.
  Kit's Default Light/Dark themes supply neutral surfaces, semantic status,
  chart and terminal ANSI colours. Interactive accents use the crane icon's
  navy `#142844`, with a lighter blue counterpart in dark mode (Huan, 2026-11-08).
  Preserve glass opacity and fonts; file/folder icons use Sandhill's palette
  below, other icons their source colours. Code syntax keeps
  Xcode's Default colours and italic rules. Kit widgets and custom UI share
  the same brand palette; do not tint all surfaces blue or cream.
  Tabs and capsules (Huan, 2026-11-14): one shape, fully round. Pane tabs are
  20px pills in a 24px track, title-bar capsules 26px; tracks and capsules
  are a faint neutral wash with no edge, and only the picked tab is raised
  (white in light mode, text colour at 20% in dark, soft shadow, no edge).
  The active project's capsule is not raised; its name is in the text colour,
  others' muted, semibold. Close buttons are one 16px circle with a muted
  (not faint) cross, readable in both themes: on the picked
  tab after its label, on others in the icon's place under the pointer
  (hover cannot change GPUI layout, so hidden buttons reserve no room);
  unsaved tabs keep their dot after the label.
  Consistency (Huan, 2026-11-15; `docs/design/ui-consistency-plan.md`,
  approved, done pane by pane): every pane's bar has the same backdrop
  whether it floats or is stacked, tabs share one padding
  (`look::TAB_PAD`), bar icons sit edge to edge, and kernel pickers are
  round capsules with the neutral wash, no edge. Labels of 12 or 12.5px
  become 13px; questions about one row are asked in that row.

## Popups and dropdowns
- Popups and dropdowns all use one small text size: 11px in the Tauri
  design, so 13px in the app after the +2px rule.
  Clicking the same dropdown trigger again closes it; outside clicks and
  Escape dismiss it.

## Icons, crane and fonts
- Icons (Huan, 2026-11-06): use Gruvbox Material Icons for files/folders,
  Lucide for controls, and Developer Icons for available integration/runtime
  logos. Remove Atom Material entirely. File and folder icons keep Gruvbox's
  shapes but not its colours (Huan, 2026-11-14): `build.py` repaints each
  Gruvbox hue in the matching Xcode Default/Apple colour (blue `#41a1c0`,
  red `#fc6a5d`, yellow `#d0bf69`, green `#78c88a`, teal `#67b7a4`, orange
  `#fd8f3f`, pink `#fc5fa3`; folder body slate `#3b4558`), so icons and code
  share one palette. The light theme uses generated `*.light.svg` versions
  darkened in OKLCH, hue kept, to 4.5:1 on `#efefef` (folder emblems 3:1, as
  they sit on the dark folder body). Other vendors' logos keep their own
  colours. Formats Gruvbox
  has no artwork for are drawn for Sandhill in its document style
  (`vendor/sandhill`, GPL like Sandhill: FASTA, FASTQ, BAM, VCF, GFF, BED,
  h5ad, mtx, HDF5, Parquet, RDS, Nextflow, WDL, CWL, index files, signal
  tracks, phylogenetic trees, Excel; Word and PowerPoint use Gruvbox's own
  art, none of them Microsoft's logos); one icon covers a family of extensions
  (`catalog.json`). A folder that is one data set (`.zarr`) shows its
  format's file icon. PLINK's binary `.bed` is told from BED intervals by
  the `.bim`/`.fam` beside it (icon) and by its first bytes (opening);
  keep generic artwork
  to the fallback. A project's default icon is the project folder's.
  Vendor pinned sources with upstream licenses and notices. Keep Sandhill's
  crane: navy `#142844` with the palette's teal feather and red crown on
  neutral white `#F5F5F7`, no cream (Huan, 2026-11-14). The Dock icon
  follows the theme on macOS 26 and later (Huan, 2026-11-14): flat, a white
  tile and navy crane in light, a navy tile and white crane in dark, tinted
  and clear looks drawn by the system with the crane light; older macOS
  keeps the navy `icon.icns`. Fonts are unchanged
  while Huan contacts the font author.
  Agent marks have no background (Huan, 2026-11-12): OpenAI's logo for Codex
  and Synthetic Sciences' mark for OpenScience, in the text colour; Posit's
  mark for Posit Assistant; Oh My Pi's gradient π; Antigravity uses his
  supplied full-colour SVG. Brand marks without a redistribution licence need
  permission documented separately from the icon-library licenses.
