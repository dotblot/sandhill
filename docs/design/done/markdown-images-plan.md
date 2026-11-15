# Plan: images in rendered Markdown

Status: approved by Huan on 2026-11-13 with both recommendations (load
`https://` images; Markdown files, notebook and Quarto text cells).

Huan: when previewing a `.md` file, show its images (and tables).

## Today

- The rendered view (`view.rs`, Markdown documents and notebook/Quarto text
  cells) uses Kit's `TextView::markdown`. Tables already render as tables.
- Images: Kit draws `data:` images and hands other URLs to GPUI's image
  loader. It never reads local files on its own, and Sandhill must not read
  project files from the Mac anyway: file reads go through the daemon, so
  remote projects work the same way. A relative path such as
  `![](figures/umap.png)` therefore shows nothing.
- Sandhill already previews PDF/SVG/PNG/JPEG/GIF/WebP/BMP/TIFF/ICO through the
  daemon (`media.rs`: `ServePage`, then the existing port tunnel for remote
  devices), and rasterizes SVG without external references.

## Change

1. Before rendering, collect each image reference in the Markdown source.
   Resolve relative paths against the document's folder (for notebook cells,
   the notebook's folder); allow absolute paths only inside the project. Any
   path outside the project, or a `..` that escapes it, is not loaded.
2. Load each allowed image through the daemon, as the image previews do, and
   give Kit the decoded image (the same `ImageSource::Image` path `data:`
   images take), so Kit never touches the file system. Same formats as the
   previews; SVG with the existing inert rasterizer.
3. Limits: at most 20 MB per image and 50 images per document; larger ones
   show a placeholder. Images are cached per path and modification time, and
   reloaded when the file changes.
4. A missing, refused or failed image shows a small placeholder box with the
   alt text and the reason in a tooltip ("Not found", "Outside the project",
   "Too large", "Not an image").
5. Web images (`https://…`): see the decision below.
6. Jupyter `attachment:` images in `.ipynb` Markdown cells come from the
   notebook itself and are shown too.

Not in scope: HTML `<img>` tags inside Markdown, videos, and following links.

## Decisions for Huan

- Web images: load `https://` images (README badges, figures hosted online),
  or never contact the internet from a preview. Recommended: load `https://`
  only.
- Scope: Markdown files only, or also notebook and Quarto text cells.
  Recommended: all three, since they share the renderer.

## Checks

- Unit tests: path resolution and the outside-the-project refusal.
- In the app: a local `.md` with a relative PNG, an SVG, a missing image and
  a table; the same on a remote project if one is connected.

## Implemented

Implemented in `gpui/src/md_images.rs`, with document-scoped state in
`workbench.rs` and minimal Markdown-document/text-cell hooks in `view.rs`.
Kit 0.9.1 has an authoritative `TextView::image_source` resolver, so rendered
copies use short internal image URLs and cached owned images rather than
base64-encoding large images into Markdown. Saved Markdown and notebook
attachments are unchanged. Inline and reference-style Markdown images work;
tables continue through Kit's existing renderer.

Relative and project-contained absolute POSIX paths are resolved on the
daemon's filesystem, including URI-escaped names; traversal outside the project
and non-HTTPS schemes are refused. Files use bounded, chunked daemon `ReadBytes`
calls with size/mtime consistency checks. PNG/JPEG/GIF/WebP/BMP/TIFF/ICO are
validated; SVG uses the shared inert byte rasterizer in `media.rs`. HTTPS uses
the existing locked `ureq` version on a background thread, with TLS-only
redirects, no cookies, a 30-second timeout and a 20 MiB body cap. Embedded
`data:` images and ipynb Markdown-cell `attachment:` bundles use the same
validation and size limit. The first 50 image references across the document
are eligible to load; excess references show a reason instead.

Loading/failure placeholders are small outlined images, with the alt text and
reason drawn in muted 13px text; the reason is also the Markdown image title
for Kit's inline image tooltip. Showing the reason in the box keeps it visible
in Kit's block-image layout too. The cache records each daemon file's mtime.
There is no existing open-document image file watcher, so changed image files
reload when the document is reloaded or closed and reopened, not live while
the document stays open. Standalone Markdown does not fetch images until its
rendered view is opened.

Checked the Kit resolver/data-image APIs and existing daemon transfer and SVG
patterns. Added in-file unit tests for relative/absolute/escaped paths, project
root boundaries, traversal and scheme refusal. Integration compile/tests and
the local/remote visual checks remain for the main agent; no app was run here.
