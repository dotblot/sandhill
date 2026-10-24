// Icons drawn for Sandhill, after the V32.2 set in docs/design/ui/icons: file
// and folder icons, language and agent chips (from the generated icons32.ts),
// and line icons for buttons.

import { type JSX, Show } from "solid-js";
import { ICONS32 } from "./icons32";

type Bodies = Readonly<Record<string, string>>;
const BIO: Bodies = ICONS32.bio;
const FOLDERS: Bodies = ICONS32.folders;
const RESOLVE = ICONS32.resolve as {
  compressed: readonly string[];
  names: Bodies;
  prefixes: Bodies;
  extensions: Bodies;
  fallback: string;
  folders: Bodies;
  folderFallback: string;
};

/** Which V32.2 icon a file or folder gets, by its name. Compressed files keep the icon of what is inside. */
export function iconKind(name: string, dir = false): string {
  const low = name.toLowerCase();
  if (dir) return RESOLVE.folders[low] ?? RESOLVE.folderFallback;
  if (RESOLVE.names[low]) return RESOLVE.names[low];
  for (const p in RESOLVE.prefixes) if (low.startsWith(p)) return RESOLVE.prefixes[p];
  const parts = low.split(".").slice(1);
  while (parts.length > 1 && RESOLVE.compressed.includes(parts[parts.length - 1])) parts.pop();
  const ext = parts[parts.length - 1];
  if (ext && RESOLVE.extensions[ext]) return RESOLVE.extensions[ext];
  return ext && RESOLVE.compressed.includes(ext) ? "archive" : RESOLVE.fallback;
}

/** The icon for a file or folder, by its name. */
export function FileIcon(props: { name: string; dir?: boolean }) {
  return (
    <svg
      class="fi"
      viewBox="0 0 16 16"
      aria-hidden="true"
      innerHTML={(props.dir ? FOLDERS : BIO)[iconKind(props.name, props.dir)]}
    />
  );
}

// Functions, so each icon gets its own elements.
const LINE_ICONS = {
  chevron: () => <path d="M5 6.5 8 9.5l3-3" />,
  chevronRight: () => <path d="M6.5 5 9.5 8l-3 3" />,
  chevronLeft: () => <path d="M9.5 5 6.5 8l3 3" />,
  zoom: () => (
    <>
      <circle cx="7" cy="7" r="4.3" />
      <path d="M10.2 10.2 13.5 13.5M7 5v4M5 7h4" />
    </>
  ),
  save: () => <path d="M8 2.5v7M5 6.5l3 3 3-3M3 12.5h10" />,
  table: () => (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="1" />
      <path d="M2.5 6.5h11M2.5 10h11M6.5 6.5V13" />
    </>
  ),
  filter: () => <path d="M2.5 3.5h11l-4.2 5v4l-2.6 1.2V8.5z" />,
  pkg: () => (
    <>
      <path d="M2.5 5 8 2.5 13.5 5v6L8 13.5 2.5 11z" />
      <path d="M2.5 5 8 7.5 13.5 5M8 7.5v6" />
    </>
  ),
  source: () => (
    <>
      <path d="M3.5 2.5h5l3 3v8h-8z" />
      <path d="M8.5 2.5v3h3" />
    </>
  ),
  plus: () => <path d="M8 3.5v9M3.5 8h9" />,
  refresh: () => (
    <>
      <path d="M12.5 8a4.5 4.5 0 1 1-1.3-3.2" />
      <path d="M12 2.5v2.8H9.2" />
    </>
  ),
  stop: () => <rect x="4" y="4" width="8" height="8" rx="1" fill="currentColor" stroke="none" />,
  play: () => <path d="M5 3.5v9l7-4.5z" fill="currentColor" stroke="none" />,
  sidebar: () => (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M6 3v10" />
    </>
  ),
  server: () => (
    <>
      <rect x="2.5" y="2.5" width="11" height="5" rx="1" />
      <rect x="2.5" y="8.5" width="11" height="5" rx="1" />
      <path d="M5 5h.01M5 11h.01" />
    </>
  ),
  laptop: () => (
    <>
      <rect x="3.5" y="3.5" width="9" height="6.5" rx="1" />
      <path d="M1.5 12.5h13" />
    </>
  ),
  folder: () => <path d="M2 4.5h4l1.5 1.5H14v6.5H2z" />,
  up: () => <path d="M8 13V3.5M4 7.5l4-4 4 4" />,
  x: () => <path d="M4.75 4.75l6.5 6.5M11.25 4.75l-6.5 6.5" />,
  leftPane: () => (
    <>
      <rect x="2" y="2.5" width="12" height="11" rx="2.2" />
      <path d="M6.2 2.5v11" />
    </>
  ),
  rightPane: () => (
    <>
      <rect x="2" y="2.5" width="12" height="11" rx="2.2" />
      <path d="M9.8 2.5v11" />
    </>
  ),
  chat: () => (
    <>
      <path d="M3.5 2.8h9a1.5 1.5 0 0 1 1.5 1.5v6a1.5 1.5 0 0 1-1.5 1.5H7.2L4.2 14v-2.2h-.7A1.5 1.5 0 0 1 2 10.3v-6a1.5 1.5 0 0 1 1.5-1.5z" />
      <path d="M5 6h6M5 8.5h4" />
    </>
  ),
  search: () => (
    <>
      <circle cx="7" cy="7" r="4.2" />
      <path d="M10.2 10.2 13.5 13.5" />
    </>
  ),
  menu: () => <path d="M2.5 4h11M2.5 8h11M2.5 12h11" />,
  trash: () => <path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 9h5.8l.6-9M7 7v4.5M9 7v4.5" />,
  cpu: () => (
    <>
      <rect x="4" y="4" width="8" height="8" rx="1.5" />
      <path d="M6.5 2v2M9.5 2v2M6.5 12v2M9.5 12v2M2 6.5h2M2 9.5h2M12 6.5h2M12 9.5h2" />
    </>
  ),
  memory: () => (
    <>
      <rect x="1.8" y="4.5" width="12.4" height="6" rx="1" />
      <path d="M4.5 6.5v2M7 6.5v2M9.5 6.5v2M12 6.5v2M3.5 10.5v1.8M12.5 10.5v1.8" />
    </>
  ),
  gpu: () => (
    <>
      <rect x="1.8" y="4" width="12.4" height="7.5" rx="1.2" />
      <circle cx="6" cy="7.75" r="2" />
      <path d="M10 6.5h2.2M10 9h2.2M3.5 11.5v1.8" />
    </>
  ),
  disk: () => (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M2 9.5h12" />
      <circle cx="11.3" cy="11.3" r=".5" />
    </>
  ),
  minimize: () => <path d="M4 8h8" />,
  maximize: () => <path d="M9.5 2.5h4v4M6.5 13.5h-4v-4M13.5 2.5 9 7M2.5 13.5 7 9" />,
  restore: () => <path d="M13 7H9V3M3 9h4v4M9 7l4.5-4.5M7 9l-4.5 4.5" />,
  monitor: () => (
    <>
      <rect x="2" y="2.5" width="12" height="8.5" rx="1.3" />
      <path d="M6 14h4M8 11v3" />
    </>
  ),
  cloud: () => <path d="M4.6 12.5a2.9 2.9 0 0 1-.4-5.8 3.9 3.9 0 0 1 7.5-.7 2.9 2.9 0 0 1 .4 6.5z" />,
  sessions: () => (
    <>
      <rect x="2.5" y="3" width="11" height="4" rx="1" />
      <rect x="2.5" y="9" width="11" height="4" rx="1" />
      <path d="M5 5h.01M5 11h.01" />
    </>
  ),
  check: () => <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />,
  help: () => (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M6.3 6.3a1.8 1.8 0 0 1 3.4.6c0 1.2-1.7 1.4-1.7 2.6M8 11.3v.01" />
    </>
  ),
  file: () => (
    <>
      <path d="M3.5 2.5h5l3 3v8h-8z" />
      <path d="M8.5 2.5v3h3" />
    </>
  ),
  command: () => <path d="M6.5 4.5 10 8l-3.5 3.5" />,
  clock: () => (
    <>
      <path d="M2.6 5.2A6 6 0 1 1 2 8" />
      <path d="M2.2 2.6v2.8h2.8M8 5v3.2l2.1 1.4" />
    </>
  ),
  gear: () => (
    <>
      <path d="M12.68 6.55 14.31 6.92 14.31 9.08 12.68 9.45 12.33 10.29 13.23 11.69 11.69 13.23 10.29 12.33 9.45 12.68 9.08 14.31 6.92 14.31 6.55 12.68 5.71 12.33 4.31 13.23 2.77 11.69 3.67 10.29 3.32 9.45 1.69 9.08 1.69 6.92 3.32 6.55 3.67 5.71 2.77 4.31 4.31 2.77 5.71 3.67 6.55 3.32 6.92 1.69 9.08 1.69 9.45 3.32 10.29 3.67 11.69 2.77 13.23 4.31 12.33 5.71Z" />
      <circle cx="8" cy="8" r="2" />
    </>
  ),
  copy: () => (
    <>
      <rect x="5" y="5" width="8" height="8" rx="1" />
      <path d="M3 10.5V3h7.5" />
    </>
  ),
  clip: () => <path d="M10.8 4.6 5.9 9.5a1.5 1.5 0 0 0 2.2 2.2l5-5a2.9 2.9 0 0 0-4.2-4.2L3.8 7.6a4.3 4.3 0 0 0 6.1 6.1l3.2-3.2" />,
  hand: () => (
    <path d="M5.6 8.4V3.6a1 1 0 0 1 2 0v3.6M7.6 7V2.7a1 1 0 0 1 2 0v4.5M9.6 7.3V3.8a1 1 0 0 1 2 0v4.6M11.6 6.6a1 1 0 0 1 2 0v3.2a4.7 4.7 0 0 1-4.7 4.7h-.6a4 4 0 0 1-3-1.4L2.8 10.2a1 1 0 0 1 1.5-1.3l1.3 1.3" />
  ),
  eye: () => (
    <>
      <path d="M1.5 8S4 3.6 8 3.6 14.5 8 14.5 8 12 12.4 8 12.4 1.5 8 1.5 8z" />
      <circle cx="8" cy="8" r="2" />
    </>
  ),
  bolt: () => <path d="M9 1.8 3.8 9h4.1l-1 5.2L12.2 7H8.1z" />,
  lock: () => (
    <>
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.2" />
      <path d="M5.5 7V5.2a2.5 2.5 0 0 1 5 0V7" />
    </>
  ),
  export: () => <path d="M8 9.5V2.5M5.2 5.2 8 2.4l2.8 2.8M4.5 7.5H3.5v6h9v-6h-1" />,
  plan: () => (
    <>
      <path d="M6.5 4.5h6.5M6.5 8h6.5M6.5 11.5h6.5M2.8 4.5l.8.8 1.4-1.5M2.8 8l.8.8 1.4-1.5" />
      <circle cx="3.8" cy="11.5" r=".9" />
    </>
  ),
  spark: () => (
    <path d="M9 1.8c.4 2.9 1.4 3.9 4.3 4.3-2.9.4-3.9 1.4-4.3 4.3-.4-2.9-1.4-3.9-4.3-4.3 2.9-.4 3.9-1.4 4.3-4.3zM4.2 9.6c.2 1.4.7 1.9 2.1 2.1-1.4.2-1.9.7-2.1 2.1-.2-1.4-.7-1.9-2.1-2.1 1.4-.2 1.9-.7 2.1-2.1z" />
  ),
  edits: () => <path d="M10.6 3.1l2.3 2.3-7.2 7.2H3.4v-2.3z" />,
  done: () => (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M5.5 8.2l1.7 1.7 3.3-3.5" />
    </>
  ),
  failed: () => (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M6 6l4 4M10 6l-4 4" />
    </>
  ),
  insert: () => <path d="M8 3v8M4.5 7.5 8 11l3.5-3.5M4 13.5h8" />,
  restart: () => (
    <>
      <path d="M12.5 8a4.5 4.5 0 1 1-1.3-3.2" />
      <path d="M12 2.5v2.8H9.2" />
    </>
  ),
  branch: () => (
    <>
      <circle cx="4.5" cy="3.5" r="1.4" />
      <circle cx="4.5" cy="12.5" r="1.4" />
      <circle cx="11.5" cy="4.5" r="1.4" />
      <path d="M4.5 4.9v6.2M11.5 5.9c0 3-7 2.2-7 5.2" />
    </>
  ),
  pull: () => <path d="M8 2.5v8M4.8 7.3 8 10.5l3.2-3.2M3.5 13.5h9" />,
  push: () => <path d="M8 13.5v-8M4.8 8.7 8 5.5l3.2 3.2M3.5 2.5h9" />,
  fetch: () => (
    <>
      <path d="M4.6 11.5a2.9 2.9 0 0 1-.4-5.8 3.9 3.9 0 0 1 7.5-.7 2.9 2.9 0 0 1 .4 6.5" />
      <path d="M8 7.5v6M6 11.5l2 2 2-2" />
    </>
  ),
  undo: () => <path d="M5.5 3.5 2.5 6.5l3 3M2.5 6.5h7a3.5 3.5 0 0 1 0 7h-2" />,
  stash: () => (
    <>
      <rect x="2.5" y="3" width="11" height="3" rx=".8" />
      <path d="M3.5 6v6.5h9V6M6.5 8.5h3" />
    </>
  ),
  more: () => (
    <>
      <circle cx="3.5" cy="8" r=".9" fill="currentColor" />
      <circle cx="8" cy="8" r=".9" fill="currentColor" />
      <circle cx="12.5" cy="8" r=".9" fill="currentColor" />
    </>
  ),
  tag: () => (
    <>
      <path d="M2.5 2.5h5.2l5.8 5.8-5.2 5.2-5.8-5.8z" />
      <circle cx="5.4" cy="5.4" r=".9" />
    </>
  ),
  globe: () => (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M2.5 8h11M8 2.5c-2.4 3-2.4 8 0 11M8 2.5c2.4 3 2.4 8 0 11" />
    </>
  ),
  minus: () => <path d="M3.5 8h9" />,
  discard: () => (
    <>
      <path d="M3.5 8a4.5 4.5 0 1 0 1.3-3.2" />
      <path d="M4 2.5v2.8h2.8" />
    </>
  ),
  open: () => <path d="M9.5 2.5h4v4M13.5 2.5 7.5 8.5M11.5 9.5v4h-9v-9h4" />,
} satisfies Record<string, () => JSX.Element>;

export type IconName = keyof typeof LINE_ICONS;

/** V32.2 ids (docs/design/ui/icons/registry.json) for the line icons, so either name works. */
export const V32_ICONS = {
  "sidebar.left": "leftPane",
  "sidebar.right": "rightPane",
  "pane.left": "sidebar",
  chat: "chat",
  search: "search",
  menu: "menu",
  more: "more",
  add: "plus",
  "device.mac": "monitor",
  "device.laptop": "laptop",
  "device.server": "server",
  "device.cloud": "cloud",
  folder: "folder",
  settings: "gear",
  "window.minimize": "minimize",
  "window.maximize": "maximize",
  "window.restore": "restore",
  "edit.undo": "undo",
  "edit.rename": "edits",
  "edit.copy": "copy",
  "doc.download": "save",
  files: "file",
  refresh: "refresh",
  filter: "filter",
  table: "table",
  share: "export",
  delete: "trash",
  package: "pkg",
  lock: "lock",
  globe: "globe",
  run: "play",
  "run.above": "insert",
  stop: "stop",
  sessions: "sessions",
  cpu: "cpu",
  memory: "memory",
  gpu: "gpu",
  disk: "disk",
  previous: "chevronLeft",
  next: "chevronRight",
  zoom: "zoom",
  help: "help",
  "git.branch": "branch",
  "git.stash": "stash",
  "git.pull": "pull",
  "git.push": "push",
  "git.tag": "tag",
  check: "check",
  "sparkle.ai": "spark",
  history: "clock",
  "context.show": "eye",
  attach: "clip",
  send: "up",
  ask: "hand",
  plan: "plan",
  auto: "bolt",
  "step.done": "done",
  "step.failed": "failed",
  close: "x",
  "chevron.down": "chevron",
  "chevron.right": "chevronRight",
  "arrow.up": "up",
} as const satisfies Record<string, IconName>;
export type V32IconId = keyof typeof V32_ICONS;

/** A line icon, drawn in the text color. */
export function Icon(props: { name: IconName | V32IconId; small?: boolean }) {
  const draw = () => LINE_ICONS[(V32_ICONS as Record<string, IconName>)[props.name] ?? (props.name as IconName)];
  return (
    <svg class="ico" classList={{ sm: props.small }} viewBox="0 0 16 16" aria-hidden="true">
      {draw()()}
    </svg>
  );
}

const LANG_CHIPS: Bodies = ICONS32.lang;
const LANG_KEY: Record<string, string> = {
  r: "r",
  python: "python",
  python3: "python",
  py: "python",
  nextflow: "nextflow",
  nf: "nextflow",
  shell: "shell",
  bash: "shell",
  sh: "shell",
  zsh: "shell",
};

/** A kernel's language: Sandhill's chips for R, Python, Nextflow and shells, or the language's first letters. */
export function LangIcon(props: { language: string }) {
  const chip = () => LANG_CHIPS[LANG_KEY[props.language.toLowerCase()] ?? ""];
  return (
    <Show
      when={chip()}
      fallback={
        <svg class="lang" viewBox="0 0 16 16" aria-hidden="true">
          <rect x="1" y="1" width="14" height="14" rx="4" fill="#6b645e" />
          <text
            x="8"
            y="11"
            text-anchor="middle"
            font-size="7.5"
            font-weight="800"
            fill="#fff"
            font-family="system-ui, sans-serif"
          >
            {props.language.slice(0, 2)}
          </text>
        </svg>
      }
    >
      {(body) => <svg class="lang" viewBox="0 0 16 16" aria-hidden="true" innerHTML={body()} />}
    </Show>
  );
}

const AGENT_CHIPS: Readonly<Record<string, { name: string; body: string }>> = ICONS32.agents;
/**
 * Official marks, by agent kind, as SVG bodies for a 16 x 16 viewBox. Empty
 * until each owner's terms for a sold product are on file; until then every
 * agent shows Sandhill's own chip. Callers never change.
 */
const BRAND_MARKS: Readonly<Record<string, string>> = {};

/** An agent's chip: Claude Code, Codex, Antigravity and omp have their own, anything else the generic one. */
export function ProviderIcon(props: { kind: string }) {
  const body = () => BRAND_MARKS[props.kind] ?? (AGENT_CHIPS[props.kind] ?? AGENT_CHIPS.generic).body;
  return <svg class="ico prov" viewBox="0 0 16 16" aria-hidden="true" innerHTML={body()} />;
}
