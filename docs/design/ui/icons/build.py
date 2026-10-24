#!/usr/bin/env python3
"""Build the Sandhill V32.2 icon set from one source.

    python3 docs/design/ui/icons/build.py

Writes, next to this script:
  system/*.svg   line icons, 16 x 16, 1.4 stroke, round caps, currentColor
  bio/*.svg      file icons, one page outline, a symbol and a family colour
  folders/*.svg  folder icons
  agents/*.svg   agent chips (Sandhill's own marks, not vendor logos)
  lang/*.svg     language and runtime chips
  registry.json  every icon: id, file, group, label, old (V31) ids
  MAPPING.md     the V31 -> V32.2 table
and refreshes the generated block in ../sandhill-layout.html and the app's
app/src/ui/icons32.ts, so the mockup and the app draw the same icons.

The system icons are read from the mockup's symbol sprite (they were drawn
for Sandhill in drafts 1-32), renamed, and topped up with the few V32.2 adds.
"""
import json, re
from pathlib import Path

HERE = Path(__file__).resolve().parent
MOCKUP = HERE.parent / "sandhill-layout.html"
APP_MODULE = HERE.parents[3] / "app" / "src" / "ui" / "icons32.ts"
SVG_ATTR = 'xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"'
LINE = 'fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"'

# ---------------------------------------------------------------- system icons
# V32.2 id, V31 id(s) in the mockup sprite, group, label
SYSTEM = [
    # window and toolbar
    ("sidebar.left", ["i-sbl"], "window", "Show or hide the files pane"),
    ("sidebar.right", ["i-sbr"], "window", "Show or hide the agent pane"),
    ("pane.left", ["i-sidebar"], "window", "Left pane (layout menu)"),
    ("pane.bottom", ["i-panel"], "window", "Bottom pane (layout menu)"),
    ("pane.right", ["i-right"], "window", "Right pane (layout menu)"),
    ("chat", ["i-chat"], "window", "Conversations"),
    ("search", ["i-search"], "window", "Search"),
    ("menu", ["i-menu"], "window", "Menu"),
    ("more", ["i-more"], "window", "More"),
    ("add", ["i-plus"], "window", "Add"),
    ("device.mac", ["i-monitor"], "window", "This Mac"),
    ("device.laptop", ["i-laptop"], "window", "Laptop"),
    ("device.server", ["i-server"], "window", "Server"),
    ("device.cloud", ["i-cloud"], "window", "Remote machine"),
    ("folder", ["i-folder"], "window", "Project folder"),
    ("settings", ["i-gear"], "window", "Settings"),
    ("window.minimize", ["i-minw"], "window", "Minimise pane"),
    ("window.maximize", ["i-maxw"], "window", "Maximise pane"),
    ("window.restore", ["i-restw"], "window", "Restore pane"),
    # edit and document
    ("edit.undo", ["i-undo"], "edit", "Undo"),
    ("edit.redo", ["i-redo"], "edit", "Redo"),
    ("edit.cut", ["i-cut"], "edit", "Cut"),
    ("edit.copy", ["i-copy"], "edit", "Copy"),
    ("edit.paste", ["i-paste"], "edit", "Paste"),
    ("edit.rename", ["i-pencil"], "edit", "Rename"),
    ("edit.compose", ["i-editsq"], "edit", "New conversation"),
    ("doc.save", ["i-savedoc"], "edit", "Save"),
    ("doc.save-all", ["i-saveall"], "edit", "Save all"),
    ("doc.download", ["i-save"], "edit", "Download / save a copy"),
    # files and navigation
    ("file.new", ["i-filenew"], "files", "New file"),
    ("folder.new", ["i-foldernew"], "files", "New folder"),
    ("files", ["i-files"], "files", "Files"),
    ("refresh", ["i-restart"], "files", "Refresh / restart"),
    ("filter", ["i-filter"], "files", "Filter"),
    ("sort", ["i-sort"], "files", "Sort"),
    ("table", ["i-table"], "files", "Table / data viewer"),
    ("outline", ["i-outline"], "files", "Outline"),
    ("share", ["i-share"], "files", "Share / export"),
    ("upload", ["i-upload"], "files", "Import / upload"),
    ("delete", ["i-trash"], "files", "Delete"),
    ("package", ["i-pkg"], "files", "Packages"),
    ("link", ["i-lockk"], "files", "Link"),
    ("lock", ["i-lock"], "files", "Locked"),
    ("star", ["i-star"], "files", "Favourite"),
    ("star.filled", ["i-starf"], "files", "Favourite (on)"),
    ("pin", ["i-pin"], "files", "Pin"),
    ("globe", ["i-globe"], "files", "Web"),
    # running code
    ("run", ["i-play"], "run", "Run"),
    ("run.above", ["i-above"], "run", "Run all above"),
    ("stop", ["i-stop"], "run", "Stop"),
    ("interrupt", [], "run", "Interrupt"),
    ("power", [], "run", "Start or shut down"),
    ("terminal", ["i-term"], "run", "Terminal"),
    ("sessions", ["i-sessions"], "run", "Sessions"),
    ("cpu", ["i-chip"], "resources", "CPU"),
    ("memory", ["i-mem"], "resources", "Memory"),
    ("gpu", ["i-gpu"], "resources", "GPU"),
    ("disk", ["i-disk"], "resources", "Disk"),
    # plots and help
    ("previous", ["i-left"], "plots", "Previous"),
    ("next", ["i-rt"], "plots", "Next"),
    ("zoom", ["i-zoom"], "plots", "Zoom"),
    ("help", ["i-help"], "plots", "Help"),
    # git
    ("git", ["i-git"], "git", "Git"),
    ("git.branch", ["i-branch"], "git", "Branch"),
    ("git.stash", ["i-stash"], "git", "Stash"),
    ("git.pull", ["i-pull"], "git", "Pull"),
    ("git.push", ["i-push"], "git", "Push"),
    ("git.tag", ["i-tag"], "git", "Tag"),
    ("check", ["i-check"], "git", "Commit / done"),
    ("sparkle", ["i-sparkle"], "git", "Write with the agent"),
    ("sparkle.ai", ["i-spark4"], "git", "Agent suggestion"),
    # agent controls
    ("agent", ["i-agent"], "agent", "Agent (generic)"),
    ("history", ["i-clock"], "agent", "History"),
    ("context.show", ["i-eye"], "agent", "Share what I'm looking at"),
    ("context.hide", ["i-eyeoff"], "agent", "Don't share what I'm looking at"),
    ("attach", ["i-clip"], "agent", "Attach (kept for the @ menu row)"),
    ("send", ["i-send"], "agent", "Send"),
    ("ask", ["i-hand"], "agent", "Ask before acting"),
    ("plan", ["i-plan"], "agent", "Plan"),
    ("auto", ["i-bolt"], "agent", "Act without asking"),
    ("think", ["i-brain"], "agent", "Thinking effort"),
    ("compact", ["i-compact"], "agent", "Compact the conversation"),
    ("skill", ["i-wand"], "agent", "Skills"),
    ("permissions", ["i-shield"], "agent", "Permissions"),
    ("step.done", ["i-ccheck"], "agent", "Step done"),
    ("step.failed", ["i-cx"], "agent", "Step failed"),
    ("warning", [], "agent", "Needs attention"),
    ("close", ["i-x"], "agent", "Close"),
    # arrows
    ("chevron.down", ["i-chev"], "arrows", "Expand"),
    ("chevron.right", ["i-chevr"], "arrows", "Collapse / go in"),
    ("arrow.up", ["i-up"], "arrows", "Up"),
]
# V32.2 adds, drawn on the same grid
NEW_SYSTEM = {
    "interrupt": '<rect x="4.5" y="3.5" width="2.2" height="9" rx=".6" fill="currentColor" stroke="none"/><rect x="9.3" y="3.5" width="2.2" height="9" rx=".6" fill="currentColor" stroke="none"/>',
    "power": '<path d="M8 2.2v5.3"/><path d="M5.1 4.4a5 5 0 1 0 5.8 0"/>',
    "warning": '<path d="M8 2.4 14 13H2z"/><path d="M8 6.6v3M8 11.4v.01"/>',
}
# old ids that are not system icons: they become agent chips (see AGENTS)
SPRITE_ELSEWHERE = {"i-claude": "agent.claude", "i-openai": "agent.codex", "i-agy": "agent.antigravity", "i-omp": "agent.omp"}

# ---------------------------------------------------------------- file icons
FAMILY = {  # family colour: >= 3:1 on the light (#eeebe7) and dark (#221f1d) panes
    "code": "#3a72b8", "docs": "#6c6fb6", "notebook": "#c4672a", "sequence": "#1b9184",
    "genomic": "#c44f3f", "data": "#8a5fb0", "table": "#4e8f4c", "workflow": "#b07d14",
    "config": "#857a64", "media": "#b4547d", "other": "#8d877d",
}
PAGE = "M4.5 1.5h4.8L13 5.2v7.8c0 .8-.7 1.5-1.5 1.5h-7c-.8 0-1.5-.7-1.5-1.5V3c0-.8.7-1.5 1.5-1.5z"
FOLD = "M9.2 1.6v2.6c0 .6.4 1 1 1h2.7"
F = "@F"  # glyph shapes filled with the family colour
# id: (family, label, glyph); glyph lines are stroked in the family colour
BIO = {
    "r": ("code", "R script", '<path d="M6 12.6V6.8h2.1a1.55 1.55 0 0 1 0 3.1H6M8.1 9.9l1.9 2.7"/>'),
    "python": ("code", "Python script", '<path d="M5 12.6V6.8h1.9a1.5 1.5 0 0 1 0 3H5M8.9 8.6l1.2 2.6M11.4 8.6l-2 4.4"/>'),
    "shell": ("code", "Shell script", '<path d="M5 7.4l2 2-2 2M8.2 12h2.8"/>'),
    "quarto": ("docs", "Quarto document", '<circle cx="8" cy="9.5" r="2.6"/><path d="M9.3 10.9l1.7 1.8"/>'),
    "rmarkdown": ("docs", "R Markdown", '<path d="M5.1 12.4V7.2h1.8a1.4 1.4 0 0 1 0 2.8H5.1M6.9 10l1.5 2.4M10.5 7v5M9.2 10.8l1.3 1.3 1.3-1.3"/>'),
    "markdown": ("docs", "Markdown", '<path d="M4.9 12.4V7.2l1.6 2.2 1.6-2.2v5.2M10.5 7v5M9.2 10.8l1.3 1.3 1.3-1.3"/>'),
    "readme": ("docs", "Read me", '<path d="M8 8.6v3.8" stroke-width="1.5"/><circle cx="8" cy="6.7" r=".85" fill="@F" stroke="none"/>'),
    "agent-notes": ("docs", "Agent instructions (CLAUDE.md, AGENTS.md)", '<path d="M8 6.3c.3 2 1 2.7 3 3-2 .3-2.7 1-3 3-.3-2-1-2.7-3-3 2-.3 2.7-1 3-3z" fill="@F"/>'),
    "notebook": ("notebook", "Jupyter notebook", '<rect x="5" y="6.6" width="6.2" height="2.4" rx=".6"/><rect x="5" y="10.2" width="6.2" height="2.4" rx=".6"/>'),
    "fasta": ("sequence", "FASTA sequences", '<path d="M5 6.8l1.4 1-1.4 1M7.8 7.8h3.4M5 10.3h6.2M5 12.5h4.4"/>'),
    "fastq": ("sequence", "FASTQ reads with qualities", '<path d="M5 7h6.2"/><path d="M5.6 12.8v-2.2M7.2 12.8V9.4M8.8 12.8v-1.4M10.4 12.8V10" stroke-width="1.3"/>'),
    "alignment": ("genomic", "Aligned reads (BAM, SAM, CRAM)", '<path d="M5 7.2h3.6M6.8 9.5h4.4M5 11.8h2.8M9.8 11.8h1.4" stroke-width="1.5"/>'),
    "variants": ("genomic", "Variants (VCF, BCF)", '<path d="M4.8 12.3h6.4M8 12.3V9.6"/><circle cx="8" cy="8.1" r="1.45" fill="@F" stroke="none"/>'),
    "annotation": ("genomic", "Gene annotation (GFF, GTF)", '<path d="M4.6 10h6.8" stroke-width="1"/><rect x="5.3" y="8.6" width="1.8" height="2.8" rx=".4" fill="@F" stroke="none"/><rect x="8.4" y="8" width="2.3" height="4" rx=".4" fill="@F" stroke="none"/>'),
    "intervals": ("genomic", "Genomic intervals (BED)", '<path d="M4.8 12.6h6.4" stroke-width="1"/><path d="M5.3 8h3M7.8 10.4h3" stroke-width="1.6"/>'),
    "r-object": ("data", "R object (RDS, RData)", '<path d="M8 6.4l2.9 1.5v3.3L8 12.7l-2.9-1.5V7.9z"/><path d="M5.1 7.9 8 9.4l2.9-1.5M8 9.4v3.3"/>'),
    "anndata": ("data", "AnnData (h5ad)", '<rect x="6.6" y="8.3" width="4.6" height="4.3" rx=".5"/><path d="M8.9 8.3v4.3M6.6 10.45h4.6" stroke-width="1"/><rect x="4.6" y="8.3" width="1.2" height="4.3" rx=".4" fill="@F" stroke="none"/><rect x="6.6" y="6.3" width="4.6" height="1.2" rx=".4" fill="@F" stroke="none"/>'),
    "hdf5": ("data", "HDF5 (h5, h5Seurat)", '<circle cx="5.8" cy="7.1" r="1"/><path d="M5.8 8.1v3.9M5.8 9.8h2.4M5.8 12h2.4"/><rect x="8.6" y="9.1" width="2.4" height="1.5" rx=".4" fill="@F" stroke="none"/><rect x="8.6" y="11.3" width="2.4" height="1.5" rx=".4" fill="@F" stroke="none"/>'),
    "sparse-matrix": ("data", "Sparse matrix (mtx)", ''.join(
        f'<circle cx="{x}" cy="{y}" r="{r}" fill="@F" stroke="none"{op}/>' for x, y, r, op in [
            (5.6, 7.4, .95, ''), (8, 7.4, .55, ' opacity=".4"'), (10.4, 7.4, .95, ''),
            (5.6, 9.8, .55, ' opacity=".4"'), (8, 9.8, .95, ''), (10.4, 9.8, .55, ' opacity=".4"'),
            (5.6, 12.2, .55, ' opacity=".4"'), (8, 12.2, .55, ' opacity=".4"'), (10.4, 12.2, .95, '')])),
    "table": ("table", "Table (CSV, TSV)", '<rect x="4.8" y="6.6" width="6.4" height="6" rx=".8"/><path d="M4.8 8.6h6.4M4.8 10.6h6.4M7.6 6.6v6" stroke-width="1"/>'),
    "parquet": ("table", "Parquet (column store)", '<rect x="4.9" y="6.6" width="1.7" height="6" rx=".5"/><rect x="7.15" y="6.6" width="1.7" height="6" rx=".5" fill="@F" fill-opacity=".45"/><rect x="9.4" y="6.6" width="1.7" height="6" rx=".5"/>'),
    "nextflow": ("workflow", "Nextflow", '<path d="M5 7.2l2 2.3-2 2.3M8.4 7.2l2 2.3-2 2.3"/>'),
    "snakemake": ("workflow", "Snakemake", '<path d="M10.8 7.4c-.6-.6-1.5-.9-2.7-.9-1.6 0-2.7.7-2.7 1.7 0 2.1 5.6 1.1 5.6 3.2 0 1-1.1 1.7-2.8 1.7-1.3 0-2.3-.4-3-1"/>'),
    "wdl": ("workflow", "WDL", '<circle cx="6" cy="7.3" r="1.1"/><circle cx="6" cy="11.9" r="1.1"/><circle cx="10.2" cy="9.6" r="1.1"/><path d="M7 7.8l2.2 1.2M7 11.4l2.2-1.2"/>'),
    "cwl": ("workflow", "CWL", '<circle cx="5.6" cy="9.6" r="1.1"/><circle cx="10.4" cy="9.6" r="1.1"/><path d="M6.7 9.6h2.4M8.3 8.7l.9.9-.9.9"/>'),
    "yaml": ("config", "YAML", '<path d="M5 7.2h3M6.6 9.5h4.4M6.6 11.8h3.4"/>'),
    "json": ("config", "JSON", '<path d="M7 6.6c-1 0-1.3.5-1.3 1.3v.7c0 .6-.4 1-.9 1 .5 0 .9.4.9 1v.7c0 .8.3 1.3 1.3 1.3M9 6.6c1 0 1.3.5 1.3 1.3v.7c0 .6.4 1 .9 1-.5 0-.9.4-.9 1v.7c0 .8-.3 1.3-1.3 1.3"/>'),
    "toml": ("config", "TOML / config", '<path d="M6.4 6.8H5.2v5.6h1.2M9.6 6.8h1.2v5.6H9.6M7.2 9.6h1.6"/>'),
    "lock": ("config", "Lock file", '<rect x="5.3" y="9" width="5.4" height="3.8" rx=".8"/><path d="M6.4 9V8a1.6 1.6 0 0 1 3.2 0v1"/>'),
    "license": ("config", "Licence", '<circle cx="8" cy="8.6" r="2"/><path d="M6.9 10.4l-.5 2.4L8 12l1.6.8-.5-2.4"/>'),
    "git": ("config", "Git settings (.gitignore)", '<circle cx="6" cy="7.2" r="1"/><circle cx="6" cy="12" r="1"/><circle cx="10.2" cy="8.4" r="1"/><path d="M6 8.2v2.8M10.2 9.4c0 1.4-1.8 1.6-3.6 2"/>'),
    "archive": ("config", "Archive (zip, tar)", '<path d="M7.4 6.4h1.2M8 7.6h1.2M7.4 8.8h1.2M8 10h1.2" stroke-width="1"/><rect x="7.2" y="10.8" width="1.8" height="2" rx=".5"/>'),
    "image": ("media", "Image or PDF", '<rect x="4.8" y="6.8" width="6.4" height="5.6" rx=".8"/><circle cx="6.6" cy="8.6" r=".7" fill="@F" stroke="none"/><path d="M5.2 12l2-1.8 1.4 1.2 1.2-1.1 1.4 1.4" stroke-width="1"/>'),
    "bibliography": ("other", "Bibliography (bib)", '<path d="M5.3 6.7h5.4M5.3 8.9h5.4M5.3 11.1h3.4" stroke-width="1"/><path d="M10 11.2l.6 1.6.6-1.6" stroke-width="1"/>'),
    "file": ("other", "Other file", ''),
}
# how a name finds its icon; compression suffixes are stripped first (.fastq.gz -> fastq)
COMPRESSED = ["gz", "bgz", "bz2", "xz", "zst"]
NAMES = {"snakefile": "snakemake", "claude.md": "agent-notes", "agents.md": "agent-notes", ".gitignore": "git",
         ".gitattributes": "git", "license": "license", "license.md": "license", "license.txt": "license", "copying": "license",
         "nextflow.config": "nextflow", "renv.lock": "lock", "uv.lock": "lock", "cargo.lock": "lock"}
PREFIXES = {"readme": "readme"}
EXTS = {
    "r": "r", "py": "python", "sh": "shell", "bash": "shell", "zsh": "shell", "qmd": "quarto", "rmd": "rmarkdown",
    "md": "markdown", "ipynb": "notebook", "fasta": "fasta", "fa": "fasta", "fna": "fasta", "faa": "fasta",
    "fastq": "fastq", "fq": "fastq", "bam": "alignment", "sam": "alignment", "cram": "alignment",
    "vcf": "variants", "bcf": "variants", "gff": "annotation", "gff3": "annotation", "gtf": "annotation",
    "bed": "intervals", "bedgraph": "intervals", "narrowpeak": "intervals", "rds": "r-object", "rdata": "r-object", "rda": "r-object",
    "h5ad": "anndata", "h5": "hdf5", "hdf5": "hdf5", "h5seurat": "hdf5", "loom": "hdf5", "mtx": "sparse-matrix",
    "csv": "table", "tsv": "table", "txt": "file", "parquet": "parquet", "nf": "nextflow", "smk": "snakemake",
    "wdl": "wdl", "cwl": "cwl", "yaml": "yaml", "yml": "yaml", "json": "json", "toml": "toml", "cfg": "toml",
    "ini": "toml", "config": "toml", "lock": "lock", "zip": "archive", "tar": "archive", "tgz": "archive",
    "png": "image", "jpg": "image", "jpeg": "image", "svg": "image", "pdf": "image", "tif": "image", "tiff": "image",
    "bib": "bibliography",
}

# ---------------------------------------------------------------- folders
FOLDER = "M1.5 4c0-.6.4-1 1-1h3.4l1.6 1.6h6c.6 0 1 .4 1 1V12c0 .6-.4 1-1 1h-11c-.6 0-1-.4-1-1z"
C = "@C"  # emblem strokes, drawn in the pane's background colour so they read as cut-outs
FOLDERS = {
    "project": ("#3a72b8", "Project root", '<path d="M11 8.6l.6 1.2 1.3.2-1 .9.3 1.3-1.2-.7-1.2.7.3-1.3-1-.9 1.3-.2z" fill="@C" stroke="none"/>'),
    "data": ("#1b9184", "data", '<ellipse cx="11" cy="9.3" rx="2" ry=".8"/><path d="M9 9.3v2.4c0 .4.9.8 2 .8s2-.4 2-.8V9.3"/>'),
    "results": ("#b07d14", "results", '<path d="M9.4 12.2v-1.4M11 12.2V9.2M12.6 12.2v-2"/>'),
    "figures": ("#c44f3f", "figures, plots", '<path d="M8.8 12.2l1.6-1.8 1.1 1.1 1.5-1.8"/>'),
    "tests": ("#8a5fb0", "tests", '<path d="M10.2 8.6v1.5l-1.4 2.3h4.4l-1.4-2.3V8.6"/>'),
    "sandhill": ("#857a64", ".sandhill (Sandhill settings)", '<circle cx="11" cy="10.5" r="1.4"/>'),
    "notebooks": ("#c4672a", "notebooks", '<rect x="9" y="8.6" width="4" height="1.4" rx=".4"/><rect x="9" y="11" width="4" height="1.4" rx=".4"/>'),
    "scripts": ("#4e8f4c", "scripts, R, src", '<path d="M9 8.8l1.4 1.4L9 11.6M11.2 12h1.8"/>'),
    "cache": ("#8d877d", "cache, tmp, work", '<circle cx="11" cy="10.4" r="1.9"/><path d="M11 9.4v1l.7.5"/>'),
    "other": ("#a09a90", "Any other folder", ''),
}
FOLDER_NAMES = {"data": "data", "raw": "data", "input": "data", "results": "results", "output": "results", "out": "results",
                "figures": "figures", "figs": "figures", "plots": "figures", "tests": "tests", "test": "tests",
                ".sandhill": "sandhill", "notebooks": "notebooks", "analysis": "notebooks", "scripts": "scripts",
                "r": "scripts", "src": "scripts", "bin": "scripts", "cache": "cache", ".cache": "cache", "tmp": "cache",
                "work": "cache", ".github": "other"}

# ---------------------------------------------------------------- chips
def chip(bg, body, title):
    return f'<rect x="1" y="1" width="14" height="14" rx="4" fill="{bg}"/>{body}'
W = 'fill="none" stroke="#fff" stroke-width="1.55" stroke-linecap="round" stroke-linejoin="round"'
AGENTS = {  # Sandhill's own marks. Swap in a vendor's mark here once its terms allow it; callers use the id only.
    "agent.claude": ("Claude Code", "#9a5b41", f'<path d="M7.2 5.6a2.6 2.6 0 1 0 0 4.8" {W}/><path d="M11.4 5.6a2.6 2.6 0 1 0 0 4.8" {W}/>', "monogram CC; official mark pending"),
    "agent.codex": ("Codex", "#3d4552", f'<path d="M7.4 5.6a2.6 2.6 0 1 0 0 4.8M9.4 5.4l2.6 5.2M12 5.4l-2.6 5.2" {W}/>', "monogram Cx; official mark pending"),
    "agent.antigravity": ("Antigravity", "#2b6f8f", f'<path d="M4 10.6 6.2 5.4l2.2 5.2M4.8 8.8h2.8M12.4 6.2a2.3 2.3 0 1 0 .2 3.4V8.4h-1.3" {W}/>', "monogram Ag; official mark pending"),
    "agent.omp": ("Oh My Pi (omp)", "#6b4fc8", f'<path d="M3.2 5.6c.6-.6 1.3-.8 2.1-.8h4.9M5.5 5v6.2M8.5 5v4.9c0 .8.4 1.3 1.1 1.3" {W}/><path d="M11.4 11.2h1.7" {W} stroke-width="1.5"/>', "Sandhill's own π mark for the omp integration; not the Oh My Pi logo"),
    "agent.generic": ("Other agent", "#6b645e", f'<path d="M8 3.6v1.6M8 10.8v1.6M3.6 8h1.6M10.8 8h1.6M5 5l1.1 1.1M9.9 9.9 11 11M5 11l1.1-1.1M9.9 6.1 11 5" {W} stroke-width="1.3"/><circle cx="8" cy="8" r="1.3" fill="#fff"/>', "generic agent"),
}
LANGS = {  # letter chips; the R, Python and Nextflow logos are not used (see LICENSES.md)
    "lang.r": ("R", "#2368b8", f'<path d="M5.8 12V4.6h2.7a2 2 0 0 1 0 4H5.8M8.6 8.6l2.2 3.4" {W} stroke-width="1.7"/>'),
    "lang.python": ("Python", "#f2c53d", '<path d="M3.6 11.6V4.8h2a1.8 1.8 0 0 1 0 3.6h-2M8.4 6.6l1.7 3.6M12.3 6.6l-2.9 6" fill="none" stroke="#22466e" stroke-width="1.55" stroke-linecap="round" stroke-linejoin="round"/>'),
    "lang.nextflow": ("Nextflow", "#1fa784", f'<path d="M3.8 11.4V7.6M3.8 8.6c0-.9.7-1.3 1.4-1.3s1.4.4 1.4 1.3v2.8M9.6 11.4V6.2c0-.9.5-1.4 1.3-1.4h.8M8.5 7.6h2.6" {W}/>'),
    "lang.shell": ("Shell", "#33363d", f'<path d="M4.4 5.4l2.6 2.6-2.6 2.6M8.4 11h3.4" {W}/>'),
}

# ---------------------------------------------------------------- build
def sprite():
    s = MOCKUP.read_text()
    return dict(re.findall(r'<symbol id="(i-[a-z0-9-]+)" viewBox="0 0 16 16">(.*?)</symbol>', s))

def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)

def bio_body(fam, glyph):
    c = FAMILY[fam]
    page = f'<path d="{PAGE}" fill="{c}" fill-opacity=".13" stroke="{c}" stroke-width="1.1" stroke-linejoin="round"/><path d="{FOLD}" fill="none" stroke="{c}" stroke-width="1.1" stroke-linejoin="round"/>'
    g = f'<g fill="none" stroke="{c}" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round">{glyph.replace("@F", c)}</g>' if glyph else ""
    return page + g

def folder_body(col, emblem, cut):
    e = f'<g fill="none" style="stroke:{cut}" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">{emblem.replace("@C", cut)}</g>' if emblem else ""
    return f'<path d="{FOLDER}" fill="{col}"/>{e}'

def main():
    sp = sprite()
    reg = {"version": "32.2", "grid": 16, "stroke": 1.4, "source": "docs/design/ui/icons/build.py", "icons": {}}
    rows = []
    for vid, old, group, label in SYSTEM:
        body = NEW_SYSTEM.get(vid) or sp[old[0]]
        write(HERE / "system" / f"{vid}.svg", f'<svg {SVG_ATTR} {LINE} aria-hidden="true">{body}</svg>\n')
        reg["icons"][vid] = {"file": f"system/{vid}.svg", "kind": "system", "group": group, "label": label, "v31": old}
        rows.append((", ".join(f"`{o}`" for o in old) or "—", f"`{vid}`", group, label + ("" if old else " (new in V32.2)")))
    for vid, (fam, label, glyph) in BIO.items():
        write(HERE / "bio" / f"{vid}.svg", f'<svg {SVG_ATTR} aria-hidden="true">{bio_body(fam, glyph)}</svg>\n')
        reg["icons"][f"file.{vid}"] = {"file": f"bio/{vid}.svg", "kind": "file", "family": fam, "color": FAMILY[fam], "label": label}
    for vid, (col, label, emblem) in FOLDERS.items():
        write(HERE / "folders" / f"{vid}.svg", f'<svg {SVG_ATTR} aria-hidden="true">{folder_body(col, emblem, "var(--tv-cut, #fff)")}</svg>\n')
        reg["icons"][f"folder.{vid}"] = {"file": f"folders/{vid}.svg", "kind": "folder", "color": col, "label": label}
    for vid, (name, bg, body, note) in AGENTS.items():
        write(HERE / "agents" / f"{vid[6:]}.svg", f'<svg {SVG_ATTR} role="img" aria-label="{name}"><title>{name}</title>{chip(bg, body, name)}</svg>\n')
        reg["icons"][vid] = {"file": f"agents/{vid[6:]}.svg", "kind": "agent", "label": name, "color": bg, "note": note, "brand": None}
    for vid, (name, bg, body) in LANGS.items():
        write(HERE / "lang" / f"{vid[5:]}.svg", f'<svg {SVG_ATTR} role="img" aria-label="{name}"><title>{name}</title>{chip(bg, body, name)}</svg>\n')
        reg["icons"][vid] = {"file": f"lang/{vid[5:]}.svg", "kind": "language", "label": name, "color": bg}
    for old, vid in SPRITE_ELSEWHERE.items():
        reg["icons"][vid]["v31"] = [old]
        rows.append((f"`{old}`", f"`{vid}`", "agent chip", AGENTS[vid][0]))
    reg["resolve"] = {"compressed": COMPRESSED, "names": NAMES, "prefixes": PREFIXES, "extensions": EXTS,
                      "fallback": "file", "folders": FOLDER_NAMES, "folderFallback": "other"}
    write(HERE / "registry.json", json.dumps(reg, indent=1) + "\n")

    # mapping table
    used = {o for _, old, _, _ in SYSTEM for o in old} | set(SPRITE_ELSEWHERE) | {"i-" + v.replace(".", "-") for v in NEW_SYSTEM}
    missing = sorted(set(sp) - used)
    assert not missing, f"V31 ids with no V32.2 icon: {missing}"
    md = ["# V31 → V32.2 icon ids", "", "Generated by `build.py`. Every symbol in the V31 sprite of",
          "`sandhill-layout.html` has a V32.2 id; the mockup keeps using the V31 ids, which the",
          "registry lists under `v31` so code can look either up.", "",
          "| V31 id | V32.2 id | Group | Used for |", "|---|---|---|---|"]
    md += [f"| {a} | {b} | {c} | {d} |" for a, b, c, d in rows]
    md += ["", "## File types", "", "| Icon | Family | Extensions and names |", "|---|---|---|"]
    for vid, (fam, label, _) in BIO.items():
        keys = [f"`.{e}`" for e, v in EXTS.items() if v == vid] + [f"`{n}`" for n, v in NAMES.items() if v == vid] + [f"`{p}*`" for p, v in PREFIXES.items() if v == vid]
        md.append(f"| `file.{vid}` | {fam} | {', '.join(keys) or 'anything else'} |")
    md += ["", f"Compressed files keep the icon of what is inside: `{'`, `'.join('.' + c for c in COMPRESSED)}` are stripped first, so `reads.fastq.gz` is a FASTQ and `matrix.mtx.gz` a sparse matrix.",
           "", "## Folders", "", "| Icon | Names |", "|---|---|"]
    for vid in FOLDERS:
        md.append(f"| `folder.{vid}` | {', '.join(f'`{n}`' for n, v in FOLDER_NAMES.items() if v == vid) or ('the project root' if vid == 'project' else 'anything else')} |")
    md += ["", "A folder's icon only changes how it looks. Sandhill does not treat files differently because of the folder they are in."]
    write(HERE / "MAPPING.md", "\n".join(md) + "\n")

    # the mockup's generated block
    js = {"bio": {k: bio_body(f, g) for k, (f, _, g) in BIO.items()},
          "folders": {k: folder_body(c, e, "var(--tv-cut)") for k, (c, _, e) in FOLDERS.items()},
          "resolve": reg["resolve"],
          "lang": {"r": chip(*LANGS["lang.r"][1:], ""), "py": chip(*LANGS["lang.python"][1:], ""),
                   "nf": chip(*LANGS["lang.nextflow"][1:], ""), "sh": chip(*LANGS["lang.shell"][1:], "")}}
    block = "/* ICONS32 (generated by icons/build.py; edit there) */\nconst ICONS32 = " + json.dumps(js, separators=(",", ":")) + ";\n/* /ICONS32 */"
    s = MOCKUP.read_text()
    s2 = re.sub(r"/\* ICONS32 .*?/\* /ICONS32 \*/", lambda m: block, s, flags=re.S)
    for old, vid in SPRITE_ELSEWHERE.items():  # chips in the sprite follow the registry
        name, bg, body, _ = AGENTS[vid]
        s2 = re.sub(rf'<symbol id="{old}" viewBox="0 0 16 16">.*?</symbol>', lambda m: f'<symbol id="{old}" viewBox="0 0 16 16">{chip(bg, body, name)}</symbol>', s2)
    for vid in NEW_SYSTEM:  # and the new system icons join the sprite
        sid = "i-" + vid.replace(".", "-")
        if f'id="{sid}"' not in s2:
            s2 = s2.replace('<symbol id="i-term"', f'<symbol id="{sid}" viewBox="0 0 16 16">{NEW_SYSTEM[vid]}</symbol>\n  <symbol id="i-term"', 1)
    if s2 != s:
        MOCKUP.write_text(s2)
    write(HERE / "preview.html", preview(reg))
    # the app's copy: the same bodies, chips keyed by the app's agent kinds and languages
    app = {"bio": js["bio"], "folders": {k: folder_body(c, e, "var(--tv-cut)") for k, (c, _, e) in FOLDERS.items()},
           "resolve": reg["resolve"],
           "lang": {k[5:]: chip(*v[1:], "") for k, v in LANGS.items()},
           "agents": {k[6:]: {"name": v[0], "body": chip(v[1], v[2], v[0])} for k, v in AGENTS.items()}}
    ts = ("// Generated by docs/design/ui/icons/build.py from the V32.2 icon set; edit there and run it.\n"
          "// SVG bodies for a 16 x 16 viewBox, and the rules that pick a file's or folder's icon.\n\n"
          f"export const ICONS32 = {json.dumps(app, indent=2)} as const;\n")
    write(APP_MODULE, ts)
    n = {k: sum(1 for v in reg["icons"].values() if v["kind"] == k) for k in ("system", "file", "folder", "agent", "language")}
    print("icons:", n, "| mockup", "updated" if s2 != s else "unchanged")

def preview(reg):
    """One page with every icon, on the light and the dark pane colours, plus the interactive states."""
    groups = {}
    for vid, v in reg["icons"].items():
        g = v["kind"] if v["kind"] != "system" else v["group"]
        groups.setdefault(g, []).append((vid, (HERE / v["file"]).read_text()))
    names = {"window": "Window & toolbar", "edit": "Edit & document", "files": "Files & navigation", "run": "Running code",
             "resources": "Resources", "plots": "Plots & help", "git": "Git", "agent": "Agent controls", "arrows": "Arrows",
             "file": "Bioinformatics files", "folder": "Folders", "language": "Languages & runtimes"}
    order = ["window", "edit", "files", "run", "resources", "plots", "git", "agent", "arrows", "file", "folder", "agent_chips", "language"]
    sec = []
    for g in order:
        items = groups.get("agent" if g == "agent_chips" else g, [])
        if g == "agent_chips": items = [i for i in items if reg["icons"][i[0]]["kind"] == "agent"]
        elif g == "agent": items = [i for i in items if reg["icons"][i[0]]["kind"] == "system"]
        cells = "".join(f'<figure title="{reg["icons"][vid]["label"]}">{svg}<figcaption>{vid}</figcaption></figure>' for vid, svg in items)
        sec.append(f'<section><h2>{names.get(g, "Agent chips")}</h2><div class="grid">{cells}</div></section>')
    st = (HERE / "system" / "run.svg").read_text()
    states = "".join(f'<button class="st {c}" {a} aria-label="Run">{st}<span>{t}</span></button>' for c, a, t in [
        ("", "", "Rest"), ("hover", "", "Hover"), ("sel", 'aria-pressed="true"', "Selected"), ("", "disabled", "Disabled")])
    sec.append(f'<section><h2>States</h2><div class="grid states">{states}</div></section>')
    body = "".join(sec)
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sandhill icons V32.2</title><style>
:root {{ color-scheme: light dark; }}
body {{ margin: 0; font: 12px/1.4 -apple-system, system-ui, sans-serif; background: #e4dfd8; }}
.cols {{ display: grid; grid-template-columns: 1fr 1fr; }}
.theme {{ padding: 18px 22px; --tv-cut: #f7f5f2; background: #f7f5f2; color: #1f1c1a; }}
.theme.dark {{ --tv-cut: #221f1d; background: #221f1d; color: #ebe6e1; }}
h1 {{ font-size: 15px; margin: 0 0 4px; }} .sub {{ color: #8a837c; margin: 0 0 10px; }}
h2 {{ font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .05em; color: #8a837c; margin: 14px 0 6px; }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(86px, 1fr)); gap: 2px; }}
figure {{ margin: 0; padding: 6px 2px; display: flex; flex-direction: column; align-items: center; gap: 4px; border-radius: 6px; }}
figure svg {{ width: 16px; height: 16px; }}
figure:hover svg {{ width: 32px; height: 32px; }}
figcaption {{ font-size: 10px; color: #8a837c; text-align: center; word-break: break-all; }}
.st {{ display: flex; align-items: center; gap: 6px; font: inherit; color: inherit; background: none; border: 0; border-radius: 6px; padding: 5px 8px; }}
.st svg {{ width: 16px; height: 16px; }}
.st.hover {{ background: color-mix(in srgb, currentColor 9%, transparent); }}
.st.sel {{ background: color-mix(in srgb, #3a72b8 16%, transparent); color: #3a72b8; }}
.dark .st.sel {{ color: #8fb4e6; }}
.st:disabled {{ opacity: .38; }}
@media (max-width: 900px) {{ .cols {{ grid-template-columns: 1fr; }} }}
</style></head><body><div class="cols">
<div class="theme"><h1>Sandhill icons · V32.2</h1><p class="sub">16 × 16 grid · 1.4 stroke · round caps · light pane</p>{body}</div>
<div class="theme dark"><h1>Sandhill icons · V32.2</h1><p class="sub">The same files on the dark pane; nothing is inverted</p>{body}</div>
</div></body></html>
"""

if __name__ == "__main__":
    main()
