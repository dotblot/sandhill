# Files

Settled decisions about which files Sandhill opens and how. Moved here
unchanged from `CLAUDE.md` on 2026-11-15; where an older plan differs, this
wins.

- Notebook formats: Jupyter `.ipynb` and Quarto `.qmd`/`.Rmd`. No marimo.
- Standalone files (Huan, 2026-11-07): Markdown `.md`/`.markdown`/`.mdown`/
  `.mkd` has source and rendered views; Shell `.sh`/`.bash`/`.zsh` and
  recognised shell shebangs have syntax highlighting, as do JSON, YAML,
  TOML and `.gitignore` (shell-style comments) beside plain `.txt` (Huan,
  2026-11-13; text files up to 5,000,000 bytes). None auto-starts a
  kernel or executes on opening. CSV/TSV first offers text or import into an
  existing live R/Python session; text is limited to 5,000,000 bytes, with
  coloured columns and a pinned logical header. File reads remain daemon-only.
  That dialog (Huan, 2026-11-14; `docs/design/done/open-files-polish-plan.md`):
  the file's icon, name, then folder · size in KB/MB · kind; two choices,
  View as text and Import into a live session (sessions listed under it,
  Copy Code and Insert into Document as links there); one primary button
  named for the choice; above 5 MB with a live session, Import is picked.
  A plain or delimited text file's colouring is chosen from a menu beside
  Wrap (Automatic, Plain text, comma/tab-separated, JSON, YAML, TOML, Shell,
  Python, R, FASTA, FASTQ, genomics table, tree, Nextflow, WDL, BibTeX),
  remembered per file on this Mac; colour only. A `.txt` whose first lines
  (2–20) have the same number of tabs opens tab-separated.
  Bounded text reads use the established chunked `read_bytes` protocol for
  compatibility with older servers. Import choices never display internal IDs.
  Every file row shows its size after the name, including empty files.
  PDF, SVG, PNG, JPEG, GIF, WebP, BMP, TIFF and ICO open as read-only middle
  document tabs with fit/zoom and PDF pagination. Use daemon file streaming
  and the existing tunnel, PDFKit/AppKit on macOS, and inert SVG rasterization
  without external references (pictures embedded in the SVG are drawn).
  Opening previews never starts a kernel. Their toolbar reads Fit, the zoom,
  Actual Size, and a hand button that toggles dragging the page or picture
  (Huan, 2026-11-14).
  Every file type in the icon catalog has a defined way to open (Huan,
  2026-11-13; `docs/design/done/file-opening-plan.md`): text kinds open with
  grammar, bioinformatics (FASTA/FASTQ, tab-separated VCF/BED/GFF/SAM/PAF…,
  Newick and Nexus trees) or
  keyword (Nextflow, WDL, BibTeX) highlighting; binary kinds ask to use the
  default app (servers: download a copy first), Microsoft Office files
  (Word, Excel, PowerPoint, with their own icons) among them. Unknown files under 5 MB that
  look like text open as plain text; text over 5 MB opens its first 5 MB
  read-only. The Files right-click menu offers "Open in Sandhill" and the
  default app for every file, Compress to .zip or .gz (.tar.gz for a
  folder) and, for files, .gz (bgzip), which tabix and samtools index; and
  Extract Here for .zip/.tar.gz/.tgz, Decompress Here for .gz/.bgz (plain or
  bgzip) (Huan, 2026-11-14). The daemon works beside the original, which
  stays; nothing is replaced, taken names are numbered ("results 2",
  "reads 2.fastq"); one item inside an archive comes out as itself,
  several in a folder named after it, without __MACOSX; entries reaching
  outside fail the whole extraction; links stay links; archives are
  offered extraction only. While one runs, the status bar shows its name, a
  progress bar of the input read, the percentage and Cancel, which stops it
  and leaves nothing half made (Huan, 2026-11-15). Rendered Markdown (files, notebook and Quarto
  text cells) shows tables and images: project images through the daemon
  (none outside the project), `https://` images, notebook attachments.
- File history (Huan, 2026-11-18): Files and the command palette open a
  tab with saved and opened versions on the left and a diff against the
  current file on the right. Restore is one undoable editor edit, not a
  save. Jupyter versions contain cell code and text, not outputs;
  restoring clears outputs for cells whose code changes. History stays
  with the daemon, keyed by project and canonical path, and does not
  follow renames. The defaults are 30 days, 100 versions per file, 2 MB
  per version and 500 MB on each computer. Identical text is stored once.
  Uninstall removes history only when asked to delete backend data.
