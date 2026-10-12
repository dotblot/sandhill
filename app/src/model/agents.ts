// What the Agents pane offers: the agent CLIs, their models, thinking effort
// and permission modes, and how to read what they report.

import type { AgentStatus, TerminalInfo } from "../backend/types";

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
    recommended: "high",
    placeholder: "Message Claude Code (runs in the terminal above)",
  },
  {
    kind: "codex",
    name: "Codex",
    program: "codex",
    plan: "Terminal · your ChatGPT plan",
    models: [{ label: "Codex's default model" }],
    efforts: ["minimal", "low", "medium", "high"],
    recommended: "medium",
    placeholder: "Ask Codex to do anything",
  },
];

export function agentKind(kind: string): AgentKind | undefined {
  return AGENT_KINDS.find((a) => a.kind === kind);
}

export interface Permission {
  id: string;
  label: string;
  detail: string;
}

export const PERMISSIONS: Permission[] = [
  { id: "supervised", label: "Supervised", detail: "Asks before commands and file changes." },
  { id: "edits", label: "Auto-accept edits", detail: "Edits files without asking; asks before other actions." },
  { id: "auto", label: "Auto", detail: "The agent's own reviewer approves or denies actions." },
  { id: "full", label: "Full access", detail: "Runs commands and edits without asking. Only for folders you can afford to lose." },
];

export function permission(id?: string): Permission {
  return PERMISSIONS.find((p) => p.id === id) ?? PERMISSIONS[0];
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
