// Renders nbformat outputs into the DOM. One renderer serves Quarto chunks
// and notebook cells. It is plain DOM rather than Solid so CodeMirror can
// host output areas as widgets without re-rendering them.

import DOMPurify from "dompurify";
import { marked } from "marked";
import { isBlobRef, type Output } from "../backend/types";
import { appendAnsi } from "./ansi";

export interface RenderContext {
  /** URL that serves a blob-backed value (plots) with the given MIME type. */
  blobUrl(hash: string, mime: string): string;
  /** Text of a blob-backed value (a very large table or log). */
  blobText(hash: string): Promise<string>;
  /** Content changed size after rendering (an image loaded, an iframe resized). */
  onResize(): void;
}

const MIME_ORDER = [
  "text/html",
  "image/svg+xml",
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "text/markdown",
  "text/latex",
  "application/json",
  "text/plain",
];

/** Streams longer than this show only their end, so a runaway loop can't freeze the UI. */
const MAX_STREAM_CHARS = 200_000;

export class OutputArea {
  readonly element: HTMLElement;
  private rendered: { output: Output; node: HTMLElement }[] = [];

  constructor(private ctx: RenderContext) {
    this.element = document.createElement("div");
    this.element.className = "output-area";
  }

  /** Re-renders only outputs that changed; unchanged outputs are the same objects. */
  update(outputs: Output[]) {
    const next: { output: Output; node: HTMLElement }[] = [];
    outputs.forEach((output, i) => {
      const prev = this.rendered[i];
      if (prev && prev.output === output) {
        next.push(prev);
        return;
      }
      const node = renderOutput(output, this.ctx);
      if (prev) prev.node.replaceWith(node);
      else this.element.append(node);
      next.push({ output, node });
    });
    for (const extra of this.rendered.slice(outputs.length)) extra.node.remove();
    this.rendered = next;
    this.element.classList.toggle("empty", outputs.length === 0);
    this.ctx.onResize();
  }

  /**
   * Lets go of plots while the area is far from view or in a hidden tab, so
   * the web view can drop their decoded pixels; `resume` loads them again.
   */
  suspend() {
    for (const img of this.element.querySelectorAll<HTMLImageElement>("img.out-image[src]")) {
      // Keep the box the plot had, so the page doesn't jump.
      if (img.offsetHeight) img.style.aspectRatio = `${img.offsetWidth} / ${img.offsetHeight}`;
      img.dataset.src = img.getAttribute("src")!;
      img.removeAttribute("src");
    }
  }

  resume() {
    for (const img of this.element.querySelectorAll<HTMLImageElement>("img.out-image[data-src]")) {
      img.setAttribute("src", img.dataset.src!);
      delete img.dataset.src;
    }
  }
}

export function renderOutput(output: Output, ctx: RenderContext): HTMLElement {
  switch (output.output_type) {
    case "stream": {
      const pre = el("pre", `out out-stream${output.name === "stderr" ? " out-stderr" : ""}`);
      let text: string = typeof output.text === "string" ? output.text : "";
      if (text.length > MAX_STREAM_CHARS) {
        const hidden = text.slice(0, text.length - MAX_STREAM_CHARS).split("\n").length - 1;
        pre.append(el("div", "out-note", `… ${hidden.toLocaleString()} earlier lines not shown`));
        text = text.slice(text.length - MAX_STREAM_CHARS);
      }
      appendAnsi(pre, text);
      return pre;
    }
    case "error": {
      const pre = el("pre", "out out-error");
      const tb: string[] = Array.isArray(output.traceback) ? output.traceback : [];
      appendAnsi(pre, tb.length ? tb.join("\n") : `${output.ename}: ${output.evalue}`);
      return pre;
    }
    case "display_data":
    case "execute_result":
      return renderBundle(output.data ?? {}, output.metadata ?? {}, ctx);
    default:
      return el("div", "out out-note", `Unsupported output: ${output.output_type}`);
  }
}

function renderBundle(data: Record<string, unknown>, metadata: Record<string, any>, ctx: RenderContext) {
  const mime = MIME_ORDER.find((m) => m in data);
  if (!mime) {
    return el("div", "out out-note", `No renderer for ${Object.keys(data).join(", ") || "empty output"}`);
  }
  const value = data[mime];
  if (mime.startsWith("image/")) return renderImage(mime, value, metadata[mime] ?? {}, ctx);
  if (isBlobRef(value)) {
    const holder = el("div", "out out-loading", "Loading output…");
    ctx.blobText(value.sandhill_blob).then(
      (text) => {
        holder.replaceWith(renderText(mime, text, ctx));
        ctx.onResize();
      },
      (err) => (holder.textContent = `Could not load output: ${err}`),
    );
    return holder;
  }
  return renderText(mime, typeof value === "string" ? value : JSON.stringify(value, null, 2), ctx);
}

function renderText(mime: string, text: string, ctx: RenderContext): HTMLElement {
  switch (mime) {
    case "text/html":
      return needsFrame(text) ? renderFrame(text, ctx) : renderInlineHtml(text);
    case "text/markdown": {
      const div = el("div", "out out-markdown");
      div.innerHTML = DOMPurify.sanitize(marked.parse(text, { async: false }) as string);
      return div;
    }
    case "application/json":
    case "text/latex":
    case "text/plain":
    default: {
      const pre = el("pre", "out out-text");
      appendAnsi(pre, text);
      return pre;
    }
  }
}

function renderImage(mime: string, value: unknown, meta: Record<string, any>, ctx: RenderContext) {
  const img = document.createElement("img");
  img.className = "out out-image";
  img.alt = "plot";
  // Set before src: plots far from view (or in a hidden tab) aren't fetched
  // or decoded until they are scrolled near.
  img.setAttribute("loading", "lazy");
  img.decoding = "async";
  if (isBlobRef(value)) {
    img.src = ctx.blobUrl(value.sandhill_blob, mime);
  } else if (typeof value === "string") {
    img.src =
      mime === "image/svg+xml"
        ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(value)}`
        : `data:${mime};base64,${value.replace(/\s/g, "")}`;
  }
  if (typeof meta.width === "number") img.width = meta.width;
  if (typeof meta.height === "number") img.height = meta.height;
  img.addEventListener("load", () => ctx.onResize());
  return img;
}

/** HTML that runs scripts or loads resources goes in a sandboxed frame. */
export function needsFrame(html: string): boolean {
  return /<(script|iframe|object|embed|link|canvas)\b|\bon[a-z]+\s*=|javascript:/i.test(html);
}

/** Static HTML (data frames, styled tables) renders inline, sanitized, with its CSS kept inside. */
function renderInlineHtml(html: string): HTMLElement {
  const host = el("div", "out out-html");
  const root = host.attachShadow({ mode: "open" });
  root.innerHTML =
    `<style>:host{display:block;overflow-x:auto;font:13px/1.45 var(--font-ui)}` +
    `table{border-collapse:collapse;border:none;font-variant-numeric:tabular-nums}` +
    `th,td{padding:3px 8px;border:none;border-bottom:1px solid var(--border);text-align:right}` +
    `thead th{border-bottom:1px solid var(--text-muted)}tbody tr:nth-child(odd){background:var(--chunk-bg)}</style>` +
    DOMPurify.sanitize(html, { FORCE_BODY: true });
  return host;
}

const FRAME_SCRIPT = `<script>
(() => {
  const post = () => parent.postMessage({ sandhillFrameHeight: document.documentElement.scrollHeight }, "*");
  new ResizeObserver(post).observe(document.documentElement);
  addEventListener("load", post);
})();
</script>`;

/**
 * Interactive HTML (htmlwidgets, plotly) runs in an iframe with scripts but
 * without same-origin access, so it can't reach the app or the daemon. The
 * frame is created only when scrolled into view.
 */
function renderFrame(html: string, ctx: RenderContext): HTMLElement {
  const holder = el("div", "out out-frame");
  holder.style.height = "320px";
  const mount = () => {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-scripts allow-popups allow-popups-to-escape-sandbox");
    frame.srcdoc = `<!doctype html><meta charset="utf-8"><style>body{margin:0;font:13px system-ui}</style>${html}${FRAME_SCRIPT}`;
    frameHeights.set(frame, (h) => {
      holder.style.height = `${Math.min(Math.max(h, 40), 4000)}px`;
      ctx.onResize();
    });
    holder.append(frame);
  };
  if (typeof IntersectionObserver === "undefined") {
    mount();
  } else {
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        io.disconnect();
        mount();
      }
    });
    io.observe(holder);
  }
  return holder;
}

const frameHeights = new WeakMap<HTMLIFrameElement, (height: number) => void>();
if (typeof window !== "undefined") {
  window.addEventListener("message", (event) => {
    const height = event.data?.sandhillFrameHeight;
    if (typeof height !== "number") return;
    for (const frame of document.querySelectorAll("iframe")) {
      if (frame.contentWindow === event.source) frameHeights.get(frame)?.(height);
    }
  });
}

function el(tag: string, className: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
