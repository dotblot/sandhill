// The machine a daemon runs on, for the left pane: CPU, memory, GPU and disk
// use, and how much memory each kernel holds. Polled while the window shows.

import { createEffect, createSignal, on, onCleanup } from "solid-js";
import type { KernelspecInfo, SessionInfo, SystemStats } from "../backend/types";
import type { Workspace } from "./workspace";

const POLL_MS = 5000;

/**
 * Asks the daemon how its machine is doing every few seconds, for as long
 * as the calling component lives, pausing while the window is hidden.
 * `path` picks the disk. Stays undefined with a daemon that can't say.
 */
export function watchMachine(ws: Workspace, path?: string): () => SystemStats | undefined {
  const [stats, setStats] = createSignal<SystemStats>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  let polling = false;
  const poll = async () => {
    clearTimeout(timer);
    if (stopped || polling) return;
    polling = true;
    try {
      if (ws.link().status === "connected") setStats(await ws.call<SystemStats>("system_stats", { path }));
    } catch {
      // An older daemon, or a dropped link; try again later.
    } finally {
      polling = false;
    }
    if (!stopped && !document.hidden) timer = setTimeout(() => void poll(), POLL_MS);
  };
  const onVisible = () => {
    if (!document.hidden) void poll();
  };
  document.addEventListener("visibilitychange", onVisible);
  // Ask as soon as the link is up, and again after a reconnect.
  createEffect(on(() => ws.link().status, (status) => status === "connected" && void poll()));
  onCleanup(() => {
    stopped = true;
    clearTimeout(timer);
    document.removeEventListener("visibilitychange", onVisible);
  });
  return stats;
}

/** "2.1 GB", "412 MB", "56 KB". */
export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = unit >= 3 && value < 10 ? 1 : 0;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

/** A whole percent of `part` in `whole`, 0 when there is no whole. */
export function percentOf(part: number, whole: number): number {
  return whole > 0 ? Math.round((part / whole) * 100) : 0;
}

/** "R", "Python", or the language as the kernel names it. */
export function languageLabel(language: string): string {
  const l = language.toLowerCase();
  if (l === "r") return "R";
  if (l === "python" || l === "python3") return "Python";
  return language.charAt(0).toUpperCase() + language.slice(1);
}

/**
 * A session as people name it: "R 4.4.1", "Python 3.12". Python shows its
 * minor version, as people say it; other languages the version the kernel gives.
 * Given the kernels, a session in an environment names it too: "Python 3.12 (omicverse)".
 */
export function sessionLabel(session: Pick<SessionInfo, "language" | "language_version"> & { kernel?: string }, specs?: KernelspecInfo[]): string {
  const name = languageLabel(session.language);
  const version = languageVersion(session);
  return withEnv(version ? `${name} ${version}` : name, session, specs);
}

/** "3.12 (omicverse)": the version, and the environment the session's kernel runs in, if any (Huan's call, 2026-11-01). */
export function versionLabel(session: Pick<SessionInfo, "language" | "language_version" | "kernel">, specs: KernelspecInfo[]): string {
  return withEnv(languageVersion(session) || languageLabel(session.language), session, specs);
}

/** The environment a session's kernel runs in, like "omicverse", if it runs in one. */
export function sessionEnv(session: { kernel?: string }, specs?: KernelspecInfo[]): string | undefined {
  return (session.kernel && specs?.find((k) => k.name === session.kernel)?.env) || undefined;
}

function withEnv(text: string, session: { kernel?: string }, specs?: KernelspecInfo[]): string {
  const env = sessionEnv(session, specs);
  return env ? `${text} (${env})` : text;
}

/**
 * "4.4.1", "3.12", or empty when the kernel didn't say. Only the number: Ark
 * says "R version 4.6.1 (2026-06-24)", as `R.version.string` does.
 */
export function languageVersion(session: Pick<SessionInfo, "language" | "language_version">): string {
  const said = session.language_version ?? "";
  const version = /\d+(?:\.\d+)+/.exec(said)?.[0] ?? said;
  return version && languageLabel(session.language) === "Python" ? version.split(".").slice(0, 2).join(".") : version;
}

/** The short name of a kernel: its environment, or what its display name has in parentheses, like "Ark". */
export function kernelShortName(kernel: string, specs: KernelspecInfo[]): string {
  const spec = specs.find((k) => k.name === kernel);
  if (!spec) return kernel;
  if (spec.env) return spec.env;
  const inner = /\(([^)]+)\)\s*$/.exec(spec.display_name);
  return inner ? inner[1] : spec.display_name;
}
