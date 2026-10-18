// Icons drawn for Sandhill: file and folder icons in muted colors, line icons
// for buttons, and filled icons for the rail. They follow the UI mockup.

import { type JSX, Show } from "solid-js";

/** Fill and darker fold color of each file icon color. */
const PALETTE = {
  sage: ["#86a77f", "#678b60"],
  mustard: ["#d8a444", "#b8862c"],
  coral: ["#d0634e", "#ad4a38"],
  slate: ["#62709f", "#4b5782"],
  teal: ["#5f9f98", "#47807a"],
  rust: ["#c97a40", "#a75f2b"],
  mauve: ["#a8687f", "#8a5065"],
  blue: ["#4f80b4", "#3c6796"],
  sand: ["#b3a487", "#94866b"],
} as const;
type Color = keyof typeof PALETTE;

/** The light color glyphs are drawn in. */
const CREAM = "#f7eedb";

type Kind =
  | "qmd"
  | "rmd"
  | "r"
  | "ipynb"
  | "py"
  | "md"
  | "claude"
  | "lock"
  | "yml"
  | "data"
  | "csv"
  | "txt";

const KIND_BY_EXT: Record<string, Kind> = {
  qmd: "qmd",
  rmd: "rmd",
  r: "r",
  ipynb: "ipynb",
  py: "py",
  md: "md",
  lock: "lock",
  yml: "yml",
  yaml: "yml",
  json: "yml",
  toml: "yml",
  csv: "csv",
  tsv: "csv",
  rds: "data",
  rdata: "data",
  rda: "data",
  h5ad: "data",
  h5: "data",
  h5mu: "data",
  loom: "data",
  mtx: "data",
  gz: "data",
  bam: "data",
  vcf: "data",
  parquet: "data",
};

const COLOR_BY_KIND: Record<Kind, Color> = {
  qmd: "slate",
  rmd: "slate",
  r: "blue",
  ipynb: "rust",
  py: "blue",
  md: "mauve",
  claude: "coral",
  lock: "mustard",
  yml: "coral",
  data: "teal",
  csv: "sage",
  txt: "sand",
};

function fileKind(name: string): Kind {
  if (name === "CLAUDE.md" || name === "AGENTS.md") return "claude";
  const dot = name.lastIndexOf(".");
  return (dot > 0 && KIND_BY_EXT[name.slice(dot + 1).toLowerCase()]) || "txt";
}

function Letters(props: { text: string; size: number; fill?: string }) {
  return (
    <text
      x="7.6"
      y={9.6 + props.size * 0.36}
      text-anchor="middle"
      font-size={String(props.size)}
      font-weight="800"
      font-family="-apple-system, system-ui, sans-serif"
      fill={props.fill ?? CREAM}
    >
      {props.text}
    </text>
  );
}

function fileGlyph(kind: Kind): JSX.Element {
  switch (kind) {
    case "qmd":
      return (
        <>
          <circle cx="7.6" cy="9.7" r="2.6" fill="none" stroke={CREAM} stroke-width="1.1" />
          <path d="M7.6 7.1v5.2M5 9.7h5.2" stroke={CREAM} stroke-width="1" />
        </>
      );
    case "rmd":
      return <Letters text="Rm" size={5} />;
    case "r":
      return <Letters text="R" size={6.6} />;
    case "ipynb":
      return (
        <>
          <ellipse
            cx="7.6"
            cy="9.7"
            rx="3.2"
            ry="1.4"
            fill="none"
            stroke={CREAM}
            stroke-width="1"
            transform="rotate(-25 7.6 9.7)"
          />
          <circle cx="7.6" cy="9.7" r="1.1" fill={CREAM} />
        </>
      );
    case "py":
      return <Letters text="Py" size={5.4} fill="#f4d57a" />;
    case "md":
      return <Letters text="M" size={6} />;
    case "claude":
      return (
        <path
          d="M7.6 6.6v6.2M4.5 9.7h6.2M5.4 7.5l4.4 4.4M9.8 7.5l-4.4 4.4"
          stroke={CREAM}
          stroke-width="1.15"
          stroke-linecap="round"
        />
      );
    case "lock":
      return (
        <>
          <rect x="5.4" y="9.2" width="4.4" height="3.4" rx=".6" fill={CREAM} />
          <path d="M6.3 9.2V8.3a1.3 1.3 0 0 1 2.6 0v.9" fill="none" stroke={CREAM} stroke-width="1" />
        </>
      );
    case "yml":
      return <Letters text="{ }" size={5} />;
    case "csv":
      return <path d="M4.8 8h5.6v4.6H4.8zM4.8 10.3h5.6M7.6 8v4.6" fill="none" stroke={CREAM} stroke-width=".9" />;
    case "txt":
      return <path d="M5 8.2h5.2M5 10h5.2M5 11.8h3.4" stroke={CREAM} stroke-width="1" stroke-linecap="round" />;
    case "data":
      return (
        <>
          <ellipse cx="7.6" cy="8.1" rx="2.5" ry=".9" fill={CREAM} />
          <path
            d="M5.1 8.1v3.6c0 .5 1.1 1 2.5 1s2.5-.5 2.5-1V8.1M5.1 9.9c0 .5 1.1 1 2.5 1s2.5-.5 2.5-1"
            fill="none"
            stroke={CREAM}
            stroke-width="1"
          />
        </>
      );
  }
}

function folderGlyph(name: string): JSX.Element {
  if (name === "data") {
    return (
      <>
        <ellipse cx="8" cy="8.7" rx="2.3" ry=".85" fill={CREAM} />
        <path d="M5.7 8.7v2.5c0 .5 1 .9 2.3.9s2.3-.4 2.3-.9V8.7" fill="none" stroke={CREAM} stroke-width="1" />
      </>
    );
  }
  if (name === "results") {
    return <path d="M5.6 12.1v-1.6M8 12.1V8.4M10.4 12.1V9.6" stroke={CREAM} stroke-width="1.4" stroke-linecap="round" />;
  }
  if (name.startsWith(".")) {
    return (
      <>
        <circle cx="8" cy="10" r="1.5" fill="none" stroke={CREAM} stroke-width="1.1" />
        <path d="M8 7.6v.8M8 11.6v.8M5.6 10h.8M9.6 10h.8" stroke={CREAM} stroke-width="1" stroke-linecap="round" />
      </>
    );
  }
  return null;
}

/** The icon for a file or folder, by its name. */
export function FileIcon(props: { name: string; dir?: boolean }) {
  if (props.dir) {
    const role: Color =
      props.name === "data" ? "teal" : props.name === "results" ? "mustard" : props.name.startsWith(".") ? "sand" : "sage";
    const [fill, fold] = PALETTE[role];
    return (
      <svg class="fi" viewBox="0 0 16 16" aria-hidden="true">
        <path d="M1.5 3.9c0-.5.4-.9.9-.9h3.4l1.5 1.5h6.4c.5 0 .8.4.8.8v1.4h-13z" fill={fold} />
        <path d="M1.5 6.3h13v6.4c0 .5-.4.8-.8.8H2.3c-.5 0-.8-.4-.8-.8z" fill={fill} />
        {folderGlyph(props.name)}
      </svg>
    );
  }
  const kind = fileKind(props.name);
  const [fill, fold] = PALETTE[COLOR_BY_KIND[kind]];
  return (
    <svg class="fi" viewBox="0 0 16 16" aria-hidden="true">
      <path d="M3.3 1.5h6.1l3.3 3.3v8.9c0 .5-.3.8-.8.8H3.3c-.5 0-.8-.3-.8-.8V2.3c0-.5.3-.8.8-.8z" fill={fill} />
      <path d="M9.4 1.5v2.5c0 .5.3.8.8.8h2.5z" fill={fold} />
      {fileGlyph(kind)}
    </svg>
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
} satisfies Record<string, () => JSX.Element>;

export type IconName = keyof typeof LINE_ICONS;

/** A line icon, drawn in the text color. */
export function Icon(props: { name: IconName; small?: boolean }) {
  return (
    <svg class="ico" classList={{ sm: props.small }} viewBox="0 0 16 16" aria-hidden="true">
      {LINE_ICONS[props.name]()}
    </svg>
  );
}

/** A kernel's language: R and Python logos, or a chip with the language's first letters. */
export function LangIcon(props: { language: string }) {
  const lang = () => props.language.toLowerCase();
  return (
    <svg class="lang" viewBox="0 0 16 16" aria-hidden="true">
      <Show
        when={lang() !== "python" && lang() !== "python3"}
        fallback={
          <>
            <path
              d="M8 1.2c-3 0-3 1.3-3 2.2v1.4h3.1v.5H3.5C2.2 5.3 1.2 6.4 1.2 8.1s1 2.9 2.3 2.9h1.2V9.2c0-1.4 1.2-2.6 2.6-2.6h3.1c1.1 0 2-.9 2-2V3.4c0-1-.9-2.2-4.4-2.2z"
              fill="#3a76ad"
            />
            <path
              d="M8 14.8c3 0 3-1.3 3-2.2v-1.4H7.9v-.5h4.6c1.3 0 2.3-1.1 2.3-2.8s-1-2.9-2.3-2.9h-1.2v1.8c0 1.4-1.2 2.6-2.6 2.6H5.6c-1.1 0-2 .9-2 2v1.2c0 1 .9 2.2 4.4 2.2z"
              fill="#f2c53d"
            />
            <circle cx="6.2" cy="2.9" r=".6" fill="#fff" />
            <circle cx="9.8" cy="13.1" r=".6" fill="#fff" />
          </>
        }
      >
        <rect x="1" y="1" width="14" height="14" rx="4" fill={lang() === "r" ? "#2368b8" : "#1fa784"} />
        <text
          x="8"
          y={lang() === "r" ? "11.6" : "11"}
          text-anchor="middle"
          font-size={lang() === "r" ? "10" : "7.5"}
          font-weight="800"
          fill="#fff"
          font-family="system-ui, sans-serif"
        >
          {lang() === "r" ? "R" : lang().slice(0, 2)}
        </text>
      </Show>
    </svg>
  );
}

/** Claude's mark, OpenAI's, or a chip for anything else. */
export function ProviderIcon(props: { kind: string }) {
  return (
    <Show
      when={props.kind === "claude"}
      fallback={
        <svg class="ico prov" viewBox="0 0 16 16" aria-hidden="true">
          <Show when={props.kind === "codex"} fallback={<rect x="3" y="3" width="10" height="10" rx="2" />}>
            <g stroke-width="1.15">
              <path d="M8 2.2 13 5.1v5.8L8 13.8 3 10.9V5.1z" />
              <path d="M8 2.2c-2.1 1.3-2.1 4.4 0 5.8M13 5.1c-2.2-1.2-4.9.4-5 2.9M13 10.9c0-2.5-2.7-4.1-5-2.9M8 13.8c2.1-1.3 2.1-4.4 0-5.8M3 10.9c2.2 1.2 4.9-.4 5-2.9M3 5.1c0 2.5 2.7 4.1 5 2.9" />
            </g>
          </Show>
        </svg>
      }
    >
      <svg class="ico prov claude" viewBox="0 0 16 16" aria-hidden="true">
        <path
          stroke-width="1.7"
          d="M8 1.6v4.2M8 10.2v4.2M1.6 8h4.2M10.2 8h4.2M3.5 3.5l3 3M9.5 9.5l3 3M3.5 12.5l3-3M9.5 6.5l3-3M5.2 2.3l1.4 3.9M9.4 9.8l1.4 3.9M2.3 10.8l3.9-1.4M9.8 6.6l3.9-1.4"
        />
      </svg>
    </Show>
  );
}
