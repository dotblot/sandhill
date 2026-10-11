// Icons drawn for Sandhill: file and folder icons in muted colors, line icons
// for buttons, and filled icons for the rail. They follow the UI mockup.

import type { JSX } from "solid-js";

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

/** The filled Files icon of the rail. */
export function FilesRailIcon() {
  return (
    <svg class="rico" viewBox="0 0 16 16" aria-hidden="true">
      <path d="M1.6 4.2v9.2c0 .6.5 1.1 1.1 1.1h7.1v-1.3H2.9v-9z" />
      <path d="M5.2 1.4h4.9l3.6 3.6v7.2c0 .6-.5 1.1-1.1 1.1H5.2c-.6 0-1.1-.5-1.1-1.1V2.5c0-.6.5-1.1 1.1-1.1z" />
      <path d="M9.8 1.6v3.1c0 .3.2.5.5.5h3.1" fill-opacity=".45" style={{ fill: "var(--bg)" }} />
    </svg>
  );
}
