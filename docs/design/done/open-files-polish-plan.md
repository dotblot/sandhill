# Opening files: dialog, colouring, Office icons, preview toolbar

Huan, 2026-11-14. Status: approved and built, with the optional automatic
tab detection for `.txt`. (The SVG preview bug from the same request is
fixed: embedded pictures were not drawn.)

## 1. Opening a delimited file

The dialog shows sizes in bytes ("12437664 bytes") and lists every action as
an equal button. Redesign, same 520px card:

- Header: the file's own icon, its name, then one muted line: folder ·
  size · kind, e.g. `data/raw · 12.4 MB · Comma-separated`. Sizes use
  KB/MB/GB with one decimal (the Files rows keep whole numbers).
- Two choices as selectable rows, one picked:
  - **View as text**: "Edit the file." or, above 5 MB, "First 5 MB,
    read-only."
  - **Import into a live session**: "Reads the whole file into R or
    Python." Picking it shows the live sessions under it (language icon,
    name, its file). With none: dimmed, "No live R or Python session.
    Start one in the Console first."
- Picked by default: View as text up to 5 MB; above 5 MB, Import when a
  session is live.
- Footer: Cancel and one primary button named for the choice ("Open Text" or
  "Import…"). Copy Code and Insert into Document become small text links
  under the session list, shown only for Import.

## 2. Choosing how a text file is coloured

A menu in the editor's action row (beside Wrap) for standalone text files
(not notebooks, Quarto, R or Python scripts, whose type decides): Plain
text, Comma-separated, Tab-separated, JSON, YAML, TOML, Shell, Python, R,
FASTA, FASTQ, genomics table (VCF/BED/GFF…), tree (Newick/Nexus),
Nextflow, WDL, BibTeX. Comma/Tab-separated also give coloured columns and
the pinned header. Colour only: nothing runs. The choice is remembered per
file on this Mac; "Automatic" (the file type's own) is the default.

## 3. Microsoft Office icons

Word (`doc docx docm dot dotx`), Excel (`xls xlsx xlsm xlsb xltx`) and
PowerPoint (`ppt pptx pptm pps ppsx potx`) get their own icons. Gruvbox
has `word.svg` and `powerpoint.svg`; it has no Excel art (its `table.svg`
is already CSV/TSV's), so Excel is drawn for Sandhill in the same document
style as the bioinformatics icons, a green grid on the page. These are
Gruvbox's/Sandhill's drawings, not Microsoft's logos. They still open only
with the default app.

## 4. Preview toolbar (PDF, SVG, images)

- The `100%` button becomes **Actual Size**, so it is not mistaken for the
  zoom percentage beside it.
- A hand button (Lucide `hand`) toggles a grab mode: the pointer becomes an
  open hand, dragging (closed hand) moves a picture or page larger than the
  pane; clicking it again restores the normal pointer. While it is on, PDF
  text cannot be selected. It is per preview and not remembered.
