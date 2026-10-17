// A terminal on screen: xterm.js showing one of the daemon's terminals, an
// agent or a shell. Made once per terminal and kept while hidden, like the
// editors, so switching tabs keeps its scrollback.

import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { type ITheme, Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import type { TerminalStore } from "../model/terminals";

const FONT = '"Maple Mono NF", ui-monospace, "SF Mono", Menlo, monospace';
const FONT_SIZE = 13;
/** Size changes while a pane is dragged are sent once it settles. */
const RESIZE_MS = 120;

// ANSI colours that read well on the panes in each mode.
const LIGHT: ITheme = {
  black: "#1f1c1a",
  red: "#c4352b",
  green: "#2f7d32",
  yellow: "#9a6300",
  blue: "#1f5fa8",
  magenta: "#8f3fa3",
  cyan: "#0e6f78",
  white: "#a39c95",
  brightBlack: "#6b645e",
  brightRed: "#d9473c",
  brightGreen: "#3d9642",
  brightYellow: "#b07a10",
  brightBlue: "#2f72c0",
  brightMagenta: "#a552b8",
  brightCyan: "#16838d",
  brightWhite: "#c9c2bb",
};
const DARK: ITheme = {
  black: "#3a3531",
  red: "#ff6b5e",
  green: "#9fd28f",
  yellow: "#f2c46d",
  blue: "#7fb4ff",
  magenta: "#d19ae8",
  cyan: "#72d0d8",
  white: "#d8d2cc",
  brightBlack: "#6c655f",
  brightRed: "#ff8a7f",
  brightGreen: "#b8e3aa",
  brightYellow: "#f7d592",
  brightBlue: "#a3c9ff",
  brightMagenta: "#e2b8f2",
  brightCyan: "#9ae0e6",
  brightWhite: "#f5f1ed",
};

export class TermView {
  readonly el: HTMLDivElement;
  readonly term: Terminal;
  private fit = new FitAddon();
  private opened = false;
  private opening?: Promise<void>;
  private id?: string;
  private detach?: () => void;
  private observer?: ResizeObserver;
  private resizeTimer?: ReturnType<typeof setTimeout>;
  private sentSize = "";
  private scheme = window.matchMedia("(prefers-color-scheme: dark)");
  private onScheme = () => (this.term.options.theme = theme());

  constructor(
    private store: TerminalStore,
    openLink: (url: string) => void,
  ) {
    this.el = document.createElement("div");
    this.el.className = "term-view";
    this.term = new Terminal({
      fontFamily: FONT,
      fontSize: FONT_SIZE,
      // The font's own line height, so box-drawing lines join up.
      lineHeight: 1,
      cursorBlink: true,
      scrollback: 10000,
      theme: theme(),
      macOptionClickForcesSelection: true,
    });
    this.term.loadAddon(this.fit);
    this.term.loadAddon(new WebLinksAddon((_event, url) => openLink(url)));
    // Shift+Enter starts a new line in Claude Code and Codex, as Option+Enter does.
    this.term.attachCustomKeyEventHandler((e) => {
      if (e.key === "Enter" && e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
        if (e.type === "keydown" && this.id) void this.store.input(this.id, "\x1b\r");
        return false;
      }
      return true;
    });
    this.term.onData((data) => {
      if (this.id) void this.store.input(this.id, data).catch(() => {});
    });
    this.scheme.addEventListener("change", this.onScheme);
  }

  /** Puts the terminal in `parent`; it fills it. Resolves once it can be used. */
  mount(parent: HTMLElement): Promise<void> {
    if (this.el.parentElement !== parent) parent.append(this.el);
    this.opening ??= (async () => {
      // xterm measures the font once, so it has to be loaded first.
      await document.fonts?.load(`${FONT_SIZE}px ${FONT}`).catch(() => {});
      this.term.open(this.el);
      this.opened = true;
      this.observer = new ResizeObserver(() => this.refit());
      this.observer.observe(this.el);
      this.refit(true);
    })();
    return this.opening;
  }

  /** Columns and rows that fit, once mounted and shown. */
  size(): { cols: number; rows: number } | undefined {
    if (!this.opened || !this.el.offsetWidth) return undefined;
    const size = this.fit.proposeDimensions();
    return size && size.cols > 0 && size.rows > 0 ? size : undefined;
  }

  /** Shows the terminal `id`: its output so far, then as it comes. */
  bind(id: string) {
    if (this.id === id) {
      this.refit(true);
      return;
    }
    this.detach?.();
    this.id = id;
    this.detach = this.store.attach(id, {
      write: (data) => this.term.write(data),
      reset: () => this.term.reset(),
    });
    this.refit(true);
  }

  focus() {
    this.term.focus();
  }

  /** The last `rows` lines on screen, as text. */
  screenText(rows = 8): string {
    const buffer = this.term.buffer.active;
    const end = buffer.baseY + this.term.rows;
    const lines: string[] = [];
    for (let i = Math.max(0, end - rows); i < end; i++) lines.push(buffer.getLine(i)?.translateToString(true) ?? "");
    return lines.join("\n");
  }

  /** Types `text` as one paste, as if dropped on the terminal. */
  paste(text: string) {
    this.term.paste(text);
  }

  dispose() {
    clearTimeout(this.resizeTimer);
    this.observer?.disconnect();
    this.detach?.();
    this.scheme.removeEventListener("change", this.onScheme);
    this.term.dispose();
    this.el.remove();
  }

  /** Fits the terminal to its box and tells the program its new size. */
  refit(now = false) {
    const size = this.size();
    if (!size) return;
    if (size.cols !== this.term.cols || size.rows !== this.term.rows) this.term.resize(size.cols, size.rows);
    clearTimeout(this.resizeTimer);
    const send = () => {
      const key = `${this.id} ${size.cols}x${size.rows}`;
      if (!this.id || key === this.sentSize) return;
      this.sentSize = key;
      void this.store.resize(this.id, size.cols, size.rows).catch(() => {});
    };
    if (now) send();
    else this.resizeTimer = setTimeout(send, RESIZE_MS);
  }
}

function theme(): ITheme {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  return {
    ...(dark ? DARK : LIGHT),
    background: v("--term") || (dark ? "#1b1816" : "#fdfcfa"),
    foreground: v("--text") || (dark ? "#ebe6e1" : "#1f1c1a"),
    cursor: v("--accent") || "#c2570c",
    cursorAccent: v("--bg") || "#ffffff",
    selectionBackground: v("--sel") || "#f6dcc4",
    scrollbarSliderBackground: dark ? "rgba(255, 255, 255, 0.12)" : "rgba(70, 45, 20, 0.14)",
    scrollbarSliderHoverBackground: dark ? "rgba(255, 255, 255, 0.22)" : "rgba(70, 45, 20, 0.26)",
    scrollbarSliderActiveBackground: dark ? "rgba(255, 255, 255, 0.3)" : "rgba(70, 45, 20, 0.34)",
  };
}
