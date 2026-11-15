# Plan: opening every file type in the icon library

Status: approved by Huan on 2026-11-13 with both recommendations (text over
5 MB opens its first 5 MB read-only; small binary-looking unknown files ask
about the default app).

Huan: open every file type the file icon library knows. Types that can be
opened as text open with highlighting; the rest ask whether to use the
system's default app. Other unknown files under 5 MB open as plain text. The
Files right-click menu offers both "open in Sandhill" and "open with the
default app".

## Today

- `notebook::format_of` opens notebooks, R/Python scripts, Markdown, shell,
  JSON/YAML/TOML/TXT/.gitignore and CSV/TSV; `media::is_media` opens PDF and
  images; HTML opens in the Viewer.
- Any other file with an extension gets "Open with Default App" (or, on a
  server, "Download to this Mac and Open"; `documents.rs::offer_external`).
  Extensionless files open as shell only with a shell shebang.
- The right-click menu has "Open" only for supported types.
- Text files open up to 5,000,000 bytes (`delimited::TEXT_LIMIT`).

## Change

### 1. Every icon-library type gets a defined way to open

| Group | Types (icon catalog) | Opens as |
|---|---|---|
| Already supported | ipynb qmd rmd r py md sh bash zsh json yaml yml toml txt csv tsv pdf png jpg jpeg svg tif tiff .gitignore | unchanged |
| Text, existing grammar | smk, Snakefile (Python); cwl (YAML); Cargo.lock, uv.lock (TOML); renv.lock (JSON); ini, cfg, config, nextflow.config (TOML-style); .gitattributes (shell-style comments) | text editor with that highlighting |
| Text, Sandhill highlighting | fa fasta fna faa (header lines, bases A/C/G/T/N in colours); fastq fq (header, sequence, `+`, quality muted); vcf bed bedgraph narrowpeak gff gff3 gtf sam mtx (`#`/`@`/`%` header lines muted, tab columns coloured like TSV) | text editor with decoration ranges, the same mechanism as CSV/TSV coloured columns |
| Text, keyword highlighting | nf (Nextflow), wdl, bib | text editor with a small keyword list (e.g. `process`, `workflow`, `input`, `output`, `script`, `task`, `call`; BibTeX `@entry` and field names), comments and strings |
| Plain text | license, copying, agents.md/claude.md/readme (Markdown), lock files not listed above | text editor, plain |
| Binary | bam cram bcf h5 h5ad h5seurat hdf5 loom parquet rds rda rdata zip tar tgz and compressed `.gz/.bgz/.bz2/.xz/.zst` | the existing "Open with Default App" / "Download to this Mac and Open" question |

All text groups use the existing bounded read (5 MB). Highlighting never
changes the saved text.

### 2. Other files

An unknown file is read first as a small prefix (8 KB) through the daemon,
as extensionless files are today. If it is under 5 MB and the prefix looks
like text (valid UTF-8, no NUL bytes), it opens as plain text. Otherwise the
default-app question appears.

### 3. Text files over 5 MB

Bioinformatics text files (VCF, FASTQ, GTF) are often far larger. See the
decision below.

### 4. Right-click menu on a file

- "Open in Sandhill" for anything Sandhill can show (all of the above except
  the binary group).
- "Open with Default App" on this Mac, or "Download to This Mac and Open" for
  a file on a server. Shown for every file, including supported ones.

Double-click keeps doing what it does after this change (open in Sandhill,
or the question for binary files).

## Decisions for Huan

- Text files over 5 MB: (a) open the first 5 MB read-only, with a note and an
  "Open with Default App" button (recommended); or (b) only the default-app
  question.
- Small unknown files that look binary: ask about the default app
  (recommended), or open them as text anyway (shows garbage; saving could
  damage the file).

## Checks

- Unit tests: type → group mapping, the text/binary prefix test, FASTA/FASTQ
  and VCF/BED decoration ranges, keyword ranges.
- In the app: open one file of each group, a 6 MB VCF, an unknown text file,
  an unknown binary file; the right-click menu on local and (if connected)
  remote projects.

## Implemented

Implemented in `gpui/src/notebook.rs`, `documents.rs`, `bio_text.rs`,
`delimited.rs`, `workbench.rs`, `find.rs`, `tree_actions.rs`, `view.rs`,
`command_palette.rs` and the `main.rs` module list. File-name matching is
case-insensitive. Unknown files use an 8 KB daemon-read prefix; binary and
large unknown files offer the default app. Known standalone text, Markdown
and delimited files over 5,000,000 bytes show a read-only prefix ending at a
complete line or UTF-8 boundary, with a direct local or remote external-open
button. Saving and Find replacement are blocked for these previews. The file
menu offers direct external opening for every file, without another question.
Bioinformatics and workflow decorations refresh after edits and theme changes;
their work is bounded to the first 262,144 bytes and 50,000 ranges per editor.
Their Wrap preferences have separate keys and default off; plain text still
defaults to wrapping. `cargo check --bin sandhill-gpui` passed, as did the
focused `bio_text::tests` (4), `documents::tests` (2) and `notebook::tests`
(24). The app was not run; the local/remote opening and menu checks above
remain manual.
