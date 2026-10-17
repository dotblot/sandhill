// What the Agents pane offers: the agent CLIs, their models, thinking effort
// and permission modes, and how to read what they report.

import type { AgentModel, AgentOptions, AgentStatus, TerminalInfo } from "../backend/types";

export interface ModelChoice {
  /** What the CLI takes; undefined leaves the CLI's own choice. */
  id?: string;
  label: string;
}

export interface AgentKind {
  kind: string;
  name: string;
  /** The CLI's program name, for the info line. */
  program: string;
  plan: string;
  models: ModelChoice[];
  /** Thinking effort levels the CLI takes, lowest first. */
  efforts: string[];
  /** The level new sessions get when none is picked: the CLI's own default. */
  recommended: string;
  placeholder: string;
}

export const AGENT_KINDS: AgentKind[] = [
  {
    kind: "claude",
    name: "Claude Code",
    program: "claude",
    plan: "Terminal · your Claude plan",
    // Aliases: the CLI maps each to the latest model of that family.
    models: [
      { id: "opus", label: "Opus" },
      { id: "sonnet", label: "Sonnet" },
      { id: "haiku", label: "Haiku" },
    ],
    efforts: ["low", "medium", "high", "xhigh", "max"],
    // What Claude Code 2.1 reports in its status line when none is given.
    recommended: "medium",
    placeholder: "Ask Claude…",
  },
  {
    kind: "codex",
    name: "Codex",
    program: "codex",
    plan: "Codex app-server · your ChatGPT plan",
    // Until a conversation starts and Codex lists its models.
    models: [{ label: "Codex's default model" }],
    efforts: ["low", "medium", "high", "xhigh"],
    recommended: "medium",
    placeholder: "Ask Codex…",
  },
];

/**
 * Agents whose chat Sandhill drives itself (Codex, through its app-server):
 * messages, modes and models go to it as commands rather than as keys typed
 * into its terminal.
 */
export function drivesChat(kind: string): boolean {
  return kind === "codex";
}

/** An agent kind with the models a running agent offers, and the efforts of the one it uses. */
export function withModels(kind: AgentKind, terminal: TerminalInfo): AgentKind {
  const models = terminal.agent?.models;
  if (!models?.length) return kind;
  const current = currentAgentModel(models, terminal);
  return {
    ...kind,
    models: models.map((m) => ({ id: m.id, label: m.label })),
    efforts: current?.efforts?.length ? current.efforts : kind.efforts,
    recommended: current?.effort ?? kind.recommended,
  };
}

function currentAgentModel(models: AgentModel[], terminal: TerminalInfo): AgentModel | undefined {
  const id = terminal.agent?.status?.model_id ?? terminal.options?.model;
  return models.find((m) => m.id === id) ?? models.find((m) => m.default);
}

export function agentKind(kind: string): AgentKind | undefined {
  return AGENT_KINDS.find((a) => a.kind === kind);
}

/** A document a session is about, as the picker stores it: relative to the project, unless outside it. */
export function documentPath(project: string, doc: string): string {
  return doc.startsWith("/") ? doc : `${project}/${doc}`;
}

/**
 * A mode: what the agent may do without asking (draft 18 uses Posit
 * Assistant's names). `permission` is what the daemon starts the agent with,
 * `claude` the matching Claude Code permission mode.
 */
export interface Mode {
  id: string;
  label: string;
  detail: string;
  permission: string;
  claude: string;
  icon: "hand" | "spark" | "bolt" | "plan" | "edits";
  /** Offered in the mode picker. */
  offered: boolean;
}

export const MODES: Mode[] = [
  { id: "normal", label: "Normal", detail: "Safe steps run. Risky ones ask first.", permission: "supervised", claude: "default", icon: "hand", offered: true },
  { id: "auto", label: "Auto", detail: "A second model reviews risky steps.", permission: "auto", claude: "auto", icon: "spark", offered: true },
  {
    id: "yolo",
    label: "YOLO",
    detail: "Everything runs without asking. Only for folders you can afford to lose.",
    permission: "full",
    claude: "bypassPermissions",
    icon: "bolt",
    offered: true,
  },
  { id: "plan", label: "Plan", detail: "Writes a plan first, then asks before carrying it out.", permission: "plan", claude: "plan", icon: "plan", offered: true },
  // Claude Code's own mode between Normal and Plan, reached with ⇧Tab in its terminal.
  { id: "edits", label: "Edits", detail: "Edits files without asking. Other steps ask first.", permission: "edits", claude: "acceptEdits", icon: "edits", offered: false },
];

/** The mode a session started in: by its `permission`. */
export function startMode(permission?: string): Mode {
  return MODES.find((m) => m.permission === permission) ?? MODES[0];
}

/** The mode for a Claude Code permission mode, as its hooks or terminal report it. */
export function claudeMode(mode: string | undefined): Mode | undefined {
  // `manual` is what newer versions call `default`.
  return MODES.find((m) => m.claude === (mode === "manual" ? "default" : mode));
}

/** The mode the agent last reported (it can change in the terminal), else the one it started in. */
export function currentMode(terminal: TerminalInfo): Mode {
  return claudeMode(terminal.agent?.mode) ?? startMode(terminal.agent?.permission ?? terminal.options?.permission);
}

/** The mode Claude Code's terminal shows under its prompt ("⏸ plan mode on"), from its last lines. */
export function screenMode(screen: string): Mode | undefined {
  const shown = [...screen.matchAll(/(manual mode|accept edits|plan mode|auto mode|bypass permissions) on/g)].pop()?.[1];
  const modes: Record<string, string> = {
    "manual mode": "default",
    "accept edits": "acceptEdits",
    "plan mode": "plan",
    "auto mode": "auto",
    "bypass permissions": "bypassPermissions",
  };
  if (shown) return claudeMode(modes[shown]);
  // Older versions show nothing in the default mode.
  return /\? for shortcuts/.test(screen) ? MODES[0] : undefined;
}

/** Whether the agent can be started again on its conversation: it said which one it is. */
export function canResume(terminal: TerminalInfo): boolean {
  if (terminal.kind === "claude") return !!terminal.agent?.session_id;
  return drivesChat(terminal.kind) && !!terminal.agent?.thread;
}

/**
 * What the agent runs with now, as it last reported: its model, effort and
 * mode can change while it runs. What it starts again with.
 */
export function liveOptions(terminal: TerminalInfo): AgentOptions {
  const options = terminal.options ?? {};
  return {
    ...options,
    model: terminal.agent?.status?.model_id ?? options.model,
    effort: effortLevel(terminal),
    permission: currentMode(terminal).permission,
  };
}

/** "Opus 5.5" once the agent says, else what the session was started with. */
export function modelLabel(terminal: TerminalInfo): string {
  const reported = terminal.agent?.status?.model;
  if (reported) return reported;
  const kind = agentKind(terminal.kind);
  const id = terminal.options?.model;
  return kind?.models.find((m) => m.id === id)?.label ?? id ?? kind?.name ?? terminal.title;
}

/** The effort level the agent last reported, else the one it started with. */
export function effortLevel(terminal: TerminalInfo): string | undefined {
  return terminal.agent?.status?.effort ?? terminal.options?.effort;
}

export function effortLabel(terminal: TerminalInfo): string | undefined {
  const effort = effortLevel(terminal);
  return effort ? effortName(effort) : undefined;
}

/** "High", "Extra high". */
export function effortName(level: string): string {
  return level === "xhigh" ? "Extra high" : capitalize(level);
}

export function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** One plan limit: how much is used and when it resets. */
export interface Usage {
  label: string;
  percent: number;
  /** Milliseconds since the epoch. */
  resetsAt?: number;
}

/** The 5-hour and weekly limits from Claude Code's status line, when it reports them. */
export function planUsage(status: AgentStatus | undefined): Usage[] {
  const limits = status?.rate_limits;
  if (!limits) return [];
  const usage: Usage[] = [];
  const add = (label: string, key: string) => {
    const limit = limits[key];
    if (typeof limit?.used_percentage !== "number") return;
    usage.push({ label, percent: Math.round(limit.used_percentage), resetsAt: time(limit.resets_at) });
  };
  add("5h", "five_hour");
  add("Week", "seven_day");
  return usage;
}

function time(value: string | number | undefined): number | undefined {
  if (value === undefined) return undefined;
  // Seconds since the epoch, or a date.
  if (typeof value === "number") return value < 1e12 ? value * 1000 : value;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? undefined : parsed;
}

/** "2h 10m", "5d 1h", "35m". */
export function timeLeft(until: number, now = Date.now()): string {
  const minutes = Math.max(0, Math.round((until - now) / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/** "12K", "200K", "1M". */
export function tokens(n: number): string {
  if (n >= 1_000_000) return `${+(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${Math.round(n / 1000)}K`;
  return String(n);
}

/**
 * What the composer sends: the text as one paste, so line breaks don't
 * submit it early, then Enter.
 */
export const SUBMIT_DELAY_MS = 60;
