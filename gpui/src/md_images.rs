//! Document-scoped Markdown images. Kit receives only owned images, never project
//! paths or web URLs. Source/attachment bytes remain untouched when saving.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use gpui_kit::component::text::TextView;
use gpui_kit::*;
use markdown::mdast::Node;
use sandhill_proto::{Call, FileBytes, MAX_READ_BYTES};

use crate::daemon::Daemon;
use crate::look::Palette;
use crate::notebook::{CellKind, Format};
use crate::workbench::{Open, Workbench};

const MAX_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGES: usize = 50;
const MAX_PIXELS: u64 = 32 * 1024 * 1024;
type Loaded = Result<Arc<Image>, &'static str>;

#[derive(Clone, Hash, PartialEq, Eq)]
enum Resource {
    File(String),
    Web(String),
    Embedded(String, u64, usize),
}

struct Reference {
    span: Range<usize>,
    alt: String,
    title: Option<String>,
    url: String,
    resource: Option<Resource>,
    refusal: Option<&'static str>,
}

#[derive(Default)]
struct CellImages {
    source: String,
    references: Vec<Reference>,
    rendered: SharedString,
    images: Arc<HashMap<String, Arc<Image>>>,
    revision: u64,
}

struct Cached {
    /// The daemon version associated with this owned image. A document reload
    /// gets a fresh cache: there is no open-document file watcher yet.
    mtime_ms: Option<i64>,
    result: Option<Loaded>,
}

#[derive(Default)]
pub(crate) struct Images {
    cells: HashMap<String, CellImages>,
    resources: HashMap<Resource, Cached>,
    dirty: bool,
    colors: Option<(Hsla, Hsla)>,
    next_revision: u64,
}

pub(crate) type Cache = Rc<RefCell<Images>>;

enum Load {
    File(String),
    Web(String),
    Data(String),
    Attachment(String),
    Failed(&'static str),
}

impl Workbench {
    /// Resolve all text cells together so the limit is per document, not per
    /// visible cell. Parsing and placeholder construction happen only on edits
    /// or completed loads; large images are never encoded into Markdown.
    pub(crate) fn sync_markdown_images(&self, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if !matches!(
            open.notebook.format,
            Format::Markdown | Format::Ipynb | Format::Quarto
        ) || open.notebook.quarto_source()
            || (open.notebook.format == Format::Markdown && !open.markdown_preview)
        {
            return;
        }
        let Some(project) = self.project.as_deref() else {
            return;
        };
        let Some(daemon) = self.daemon() else { return };
        let cache = open.md_images.clone();
        let mut state = cache.borrow_mut();
        let mut changed = false;
        let mut ids = HashSet::new();
        for (cell, editor) in open.notebook.cells.iter().zip(&open.editors) {
            // A standalone Markdown file is one source cell, not a text cell.
            if cell.kind != CellKind::Text && open.notebook.format != Format::Markdown {
                continue;
            }
            ids.insert(cell.id.clone());
            let source = editor.read(cx).value().to_string();
            if state
                .cells
                .get(&cell.id)
                .is_some_and(|old| old.source == source)
            {
                continue;
            }
            let references = references(&source);
            state.next_revision += 1;
            let revision = state.next_revision;
            state.cells.insert(
                cell.id.clone(),
                CellImages {
                    source,
                    references,
                    revision,
                    ..CellImages::default()
                },
            );
            changed = true;
        }
        if state.cells.len() != ids.len() {
            state.cells.retain(|id, _| ids.contains(id));
            changed = true;
        }
        let mut requests = Vec::new();
        if changed {
            let mut used = HashSet::new();
            let mut count = 0;
            let Images {
                cells, resources, ..
            } = &mut *state;
            for cell in &open.notebook.cells {
                let Some(rendered) = cells.get_mut(&cell.id) else {
                    continue;
                };
                for (ordinal, reference) in rendered.references.iter_mut().enumerate() {
                    count += 1;
                    reference.resource = None;
                    reference.refusal = None;
                    if count > MAX_IMAGES {
                        reference.refusal = Some("Too many images (limit 50)");
                        continue;
                    }
                    let resource = if reference.url.starts_with("https://") {
                        Resource::Web(reference.url.clone())
                    } else if reference.url.starts_with("data:")
                        || reference.url.starts_with("attachment:")
                    {
                        Resource::Embedded(cell.id.clone(), rendered.revision, ordinal)
                    } else {
                        match project_path(&reference.url, open.notebook.folder(), project) {
                            Ok(path) => Resource::File(path),
                            Err(reason) => {
                                reference.refusal = Some(reason);
                                continue;
                            }
                        }
                    };
                    used.insert(resource.clone());
                    reference.resource = Some(resource.clone());
                    if !resources.contains_key(&resource) {
                        let load = match &resource {
                            Resource::Web(url) => Load::Web(url.clone()),
                            Resource::File(path) => Load::File(path.clone()),
                            Resource::Embedded(_, _, _) => {
                                match reference.url.strip_prefix("attachment:") {
                                    Some(name) => attachment(cell.attachments(), name),
                                    None => Load::Data(reference.url.clone()),
                                }
                            }
                        };
                        requests.push((resource, load));
                    }
                }
            }
            // Embedded data is tied to the source revision, unlike shared files.
            state.resources.retain(|key, _| used.contains(key));
            requests.retain(|(key, _)| !state.resources.contains_key(key));
            let mut queued = HashSet::new();
            requests.retain(|(key, _)| queued.insert(key.clone()));
            for (key, _) in &requests {
                state.resources.insert(
                    key.clone(),
                    Cached {
                        mtime_ms: None,
                        result: None,
                    },
                );
            }
            state.dirty = true;
        }
        let p = *Palette::get(cx);
        if state.colors != Some((p.muted, p.line)) {
            state.colors = Some((p.muted, p.line));
            state.dirty = true;
        }
        if state.dirty {
            let Images {
                cells, resources, ..
            } = &mut *state;
            for cell in cells.values_mut() {
                rebuild(cell, resources, &p);
            }
            state.dirty = false;
        }
        drop(state);
        if requests.is_empty() {
            return;
        }
        let rt = self.rt.clone();
        let pending_cache = Rc::downgrade(&cache);
        cx.spawn(async move |this, cx| {
            for (key, load) in requests {
                let Some(cache) = pending_cache.upgrade() else {
                    break;
                };
                if !cache.borrow().resources.contains_key(&key) {
                    continue;
                }
                let result = match load {
                    Load::File(path) => read_file(&daemon, path).await,
                    load => match rt.spawn_blocking(move || load_inline(load)).await {
                        Ok(result) => result.map(|image| (None, image)),
                        Err(_) => Err("Could not load image"),
                    },
                };
                // Decode/rasterize file bytes on tokio, not the UI executor.
                let result = match result {
                    Ok((mtime, bytes)) => match rt.spawn_blocking(move || decode(bytes)).await {
                        Ok(result) => result.map(|image| (mtime, image)),
                        Err(_) => Err("Could not load image"),
                    },
                    Err(reason) => Err(reason),
                };
                let mut state = cache.borrow_mut();
                if let Some(entry) = state.resources.get_mut(&key) {
                    match result {
                        Ok((mtime, image)) => {
                            entry.mtime_ms = mtime;
                            entry.result = Some(Ok(image));
                        }
                        Err(reason) => entry.result = Some(Err(reason)),
                    }
                    state.dirty = true;
                }
                drop(state);
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

/// The authoritative Kit resolver also denies images introduced by raw HTML or
/// parser differences; it never falls back to GPUI's URL loader.
pub(crate) fn markdown(open: &Open, ix: usize, id: impl Into<ElementId>, cx: &App) -> TextView {
    let state = open.md_images.borrow();
    let cell = state.cells.get(&open.notebook.cells[ix].id);
    let text = cell
        .map(|cell| cell.rendered.clone())
        .unwrap_or_else(|| open.editors[ix].read(cx).value().to_string().into());
    let images = cell.map(|cell| cell.images.clone()).unwrap_or_default();
    static DENIED: std::sync::LazyLock<Arc<Image>> =
        std::sync::LazyLock::new(|| placeholder("Image", "Not loaded", "#888888", "#aaaaaa"));
    let denied = DENIED.clone();
    TextView::markdown(id, text).image_source(move |url| {
        let url: &str = url.as_ref();
        ImageSource::Image(images.get(url).unwrap_or(&denied).clone())
    })
}

fn references(source: &str) -> Vec<Reference> {
    fn walk<'a>(node: &'a Node, nodes: &mut Vec<&'a Node>) {
        nodes.push(node);
        if let Some(children) = node.children() {
            for child in children {
                walk(child, nodes);
            }
        }
    }
    let Ok(root) = markdown::to_mdast(source, &markdown::ParseOptions::gfm()) else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    walk(&root, &mut nodes);
    let mut definitions = HashMap::new();
    for node in &nodes {
        if let Node::Definition(definition) = node {
            definitions
                .entry(&definition.identifier)
                .or_insert(definition);
        }
    }
    nodes
        .iter()
        .filter_map(|node| {
            let (alt, url, title) = match node {
                Node::Image(image) => (&image.alt, &image.url, &image.title),
                Node::ImageReference(image) => {
                    let definition = definitions.get(&image.identifier)?;
                    (&image.alt, &definition.url, &definition.title)
                }
                _ => return None,
            };
            let position = node.position()?;
            Some(Reference {
                span: position.start.offset..position.end.offset,
                alt: alt.clone(),
                url: url.clone(),
                title: title.clone(),
                resource: None,
                refusal: None,
            })
        })
        .collect()
}

fn rebuild(cell: &mut CellImages, resources: &HashMap<Resource, Cached>, p: &Palette) {
    let mut text = String::with_capacity(cell.source.len());
    let mut images = HashMap::new();
    let mut start = 0;
    for (ix, reference) in cell.references.iter().enumerate() {
        text.push_str(&cell.source[start..reference.span.start]);
        let result = reference.refusal.map(Err).or_else(|| {
            reference
                .resource
                .as_ref()
                .and_then(|key| resources.get(key))
                .and_then(|entry| entry.result.clone())
        });
        let (image, title) = match result {
            Some(Ok(image)) => (image, reference.title.as_deref().unwrap_or(&reference.alt)),
            result => {
                let reason = match result {
                    Some(Err(reason)) => reason,
                    _ => "Loading…",
                };
                (
                    placeholder(&reference.alt, reason, &color(p.muted), &color(p.line)),
                    reason,
                )
            }
        };
        let url = format!("sandhill-image-{ix}");
        images.insert(url.clone(), image);
        text.push_str(&format!(
            "![{}]({url} \"{}\")",
            escape(&reference.alt),
            escape(title)
        ));
        start = reference.span.end;
    }
    text.push_str(&cell.source[start..]);
    cell.rendered = text.into();
    cell.images = Arc::new(images);
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace(['\r', '\n'], " ")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('"', "\\\"")
}

fn color(color: Hsla) -> String {
    let color = color.to_rgb();
    format!(
        "#{:02x}{:02x}{:02x}",
        (color.r * 255.) as u8,
        (color.g * 255.) as u8,
        (color.b * 255.) as u8
    )
}

fn placeholder(alt: &str, reason: &str, foreground: &str, border: &str) -> Arc<Image> {
    let label = if alt.is_empty() {
        reason.to_string()
    } else {
        format!("{alt} — {reason}")
    };
    let label: String = label.chars().take(90).collect();
    let width = (label.chars().count() * 8 + 20).clamp(100, 600);
    let label = label
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    Arc::new(Image::from_bytes(ImageFormat::Svg, format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="30"><rect x="0.5" y="0.5" width="{}" height="29" rx="4" fill="none" stroke="{border}"/><text x="10" y="20" font-family="sans-serif" font-size="13" fill="{foreground}">{label}</text></svg>"#, width - 1).into_bytes()))
}

fn attachment(attachments: &serde_json::Value, name: &str) -> Load {
    let Ok(name) = percent_decode(name) else {
        return Load::Failed("Not found");
    };
    let Some(bundle) = attachments
        .get(&name)
        .and_then(serde_json::Value::as_object)
    else {
        return Load::Failed("Not found");
    };
    for mime in [
        "image/png",
        "image/jpeg",
        "image/svg+xml",
        "image/gif",
        "image/webp",
        "image/bmp",
        "image/tiff",
        "image/x-icon",
        "image/vnd.microsoft.icon",
    ] {
        if let Some(value) = bundle.get(mime) {
            let data = crate::notebook::multiline(value);
            if data.len() as u64 > MAX_BYTES.div_ceil(3) * 4 + 1024 {
                return Load::Failed("Too large");
            }
            return Load::Attachment(data);
        }
    }
    Load::Failed("Not an image")
}

fn load_inline(load: Load) -> Result<Vec<u8>, &'static str> {
    let bytes = match load {
        Load::Web(url) => {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .https_only(true)
                .timeout_global(Some(Duration::from_secs(30)))
                .max_redirects(5)
                .build()
                .into();
            let mut response = agent.get(&url).call().map_err(|_| "Could not load image")?;
            if response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|size| size > MAX_BYTES)
            {
                return Err("Too large");
            }
            let mut bytes = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "Could not load image")?;
            bytes
        }
        Load::Data(url) => {
            if url.len() as u64 > MAX_BYTES * 3 + 1024 {
                return Err("Too large");
            }
            let data = data_url::DataUrl::process(&url).map_err(|_| "Not an image")?;
            if data.mime_type().type_ != "image" {
                return Err("Not an image");
            }
            data.decode_to_vec().map_err(|_| "Not an image")?.0
        }
        Load::Attachment(data) => base64::engine::general_purpose::STANDARD
            .decode(data.split_whitespace().collect::<String>())
            .map_err(|_| "Not an image")?,
        Load::Failed(reason) => return Err(reason),
        Load::File(_) => unreachable!("files use daemon reads"),
    };
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Too large");
    }
    Ok(bytes)
}

async fn read_file(daemon: &Daemon, path: String) -> Result<(Option<i64>, Vec<u8>), &'static str> {
    let before: FileBytes = daemon
        .call(Call::ReadBytes {
            path: path.clone(),
            offset: 0,
            limit: Some(0),
        })
        .await
        .map_err(|_| "Not found")?;
    if before.size > MAX_BYTES {
        return Err("Too large");
    }
    let mut bytes = Vec::with_capacity(before.size as usize);
    while (bytes.len() as u64) < before.size {
        let offset = bytes.len() as u64;
        let count = (before.size - offset).min(MAX_READ_BYTES);
        let chunk: FileBytes = daemon
            .call(Call::ReadBytes {
                path: path.clone(),
                offset,
                limit: Some(count),
            })
            .await
            .map_err(|_| "Could not load image")?;
        if chunk.path != before.path
            || chunk.offset != offset
            || chunk.size != before.size
            || chunk.mtime_ms != before.mtime_ms
        {
            return Err("Image changed; reopen document");
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(chunk.data.trim())
            .map_err(|_| "Could not load image")?;
        if data.len() as u64 != count {
            return Err("Image changed; reopen document");
        }
        bytes.extend_from_slice(&data);
    }
    let after: FileBytes = daemon
        .call(Call::ReadBytes {
            path,
            offset: 0,
            limit: Some(0),
        })
        .await
        .map_err(|_| "Not found")?;
    if after.path != before.path || after.size != before.size || after.mtime_ms != before.mtime_ms {
        return Err("Image changed; reopen document");
    }
    Ok((Some(before.mtime_ms), bytes))
}

fn decode(bytes: Vec<u8>) -> Loaded {
    if let Ok(format) = image::guess_format(&bytes) {
        let gpui_format = match format {
            image::ImageFormat::Png => ImageFormat::Png,
            image::ImageFormat::Jpeg => ImageFormat::Jpeg,
            image::ImageFormat::Gif => ImageFormat::Gif,
            image::ImageFormat::WebP => ImageFormat::Webp,
            image::ImageFormat::Bmp => ImageFormat::Bmp,
            image::ImageFormat::Tiff => ImageFormat::Tiff,
            image::ImageFormat::Ico => ImageFormat::Ico,
            _ => return Err("Not an image"),
        };
        let reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
        let (width, height) = reader.into_dimensions().map_err(|_| "Not an image")?;
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err("Too large");
        }
        image::ImageReader::with_format(Cursor::new(&bytes), format)
            .decode()
            .map_err(|_| "Not an image")?;
        return Ok(Arc::new(Image::from_bytes(gpui_format, bytes)));
    }
    crate::media::rasterize_svg_bytes(&bytes)
        .map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)))
        .map_err(|_| "Not an image")
}

/// Lexical POSIX resolution: remote paths are never interpreted on the Mac.
/// Reject a traversal at the project boundary, even if later components would
/// return inside it. URI escaping is decoded before checking that boundary.
fn project_path(url: &str, folder: &str, project: &str) -> Result<String, &'static str> {
    fn absolute(path: &str) -> Option<Vec<&str>> {
        if !path.starts_with('/') {
            return None;
        }
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                _ => parts.push(part),
            }
        }
        Some(parts)
    }
    if url.starts_with("//") || url.starts_with('~') || url.contains(':') {
        return Err("Only HTTPS or project images are allowed");
    }
    let url = url.split(['?', '#']).next().unwrap_or_default();
    let path = percent_decode(url).map_err(|_| "Invalid image path")?;
    if path.is_empty() || path.contains(['\0', '\\']) {
        return Err("Invalid image path");
    }
    let root = absolute(project).ok_or("Outside the project")?;
    let mut parts = if path.starts_with('/') {
        Vec::new()
    } else {
        absolute(folder).ok_or("Outside the project")?
    };
    if !path.starts_with('/') && !parts.starts_with(&root) {
        return Err("Outside the project");
    }
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() <= root.len() {
                    return Err("Outside the project");
                }
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    if !parts.starts_with(&root) || parts.len() <= root.len() {
        return Err("Outside the project");
    }
    Ok(format!("/{}", parts.join("/")))
}

fn percent_decode(text: &str) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hi = (input.next().ok_or(())? as char).to_digit(16).ok_or(())?;
            let lo = (input.next().ok_or(())? as char).to_digit(16).ok_or(())?;
            bytes.push((hi * 16 + lo) as u8);
        } else {
            bytes.push(byte)
        }
    }
    String::from_utf8(bytes).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::project_path;

    #[test]
    fn resolves_relative_absolute_and_escaped_image_paths() {
        assert_eq!(
            project_path("figures/a.png", "/p/docs", "/p").unwrap(),
            "/p/docs/figures/a.png"
        );
        assert_eq!(
            project_path("../a%20b.svg#figure", "/p/docs", "/p/").unwrap(),
            "/p/a b.svg"
        );
        assert_eq!(
            project_path("/p/assets/a.png", "/p/docs", "/p").unwrap(),
            "/p/assets/a.png"
        );
        assert_eq!(project_path("a.png", "/", "/").unwrap(), "/a.png");
    }

    #[test]
    fn refuses_outside_paths_traversal_and_nonproject_schemes() {
        for path in [
            "../../secret.png",
            "%2e%2e/%2e%2e/secret.png",
            "/project-other/a.png",
            "/p",
            "../..//p/a.png",
            "file:///p/a.png",
            "http://example.com/a.png",
            "//example.com/a.png",
            "~/a.png",
            "a%00.png",
            "a%5c.png",
            "a%zz.png",
        ] {
            assert!(project_path(path, "/p/docs", "/p").is_err(), "{path}");
        }
        assert!(project_path("/p/../p/a.png", "/p/docs", "/p").is_err());
        assert!(project_path("a.png", "/elsewhere", "/p").is_err());
    }
}
