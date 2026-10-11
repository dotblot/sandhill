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
  file: () => (
    <>
      <path d="M3.5 2.5h5l3 3v8h-8z" />
      <path d="M8.5 2.5v3h3" />
    </>
  ),
  command: () => <path d="M6.5 4.5 10 8l-3.5 3.5" />,
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
