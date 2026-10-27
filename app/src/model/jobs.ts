// Jobs: long scripts and pipelines the daemon runs in the background, each in
// a terminal of its own (see `start_job`). These say which files can run as
// jobs, how a job stands, and what to tell the user when one ends.

import type { TerminalInfo } from "../backend/types";

/** Files that run as jobs with the program for their kind; executables run too, which a name can't tell. */
export function isJobScript(path: string): boolean {
  const name = path.split("/").pop() ?? "";
  return /\.(r|py|sh|bash|nf|smk)$/i.test(name) || name === "Snakefile";
}

export type JobState = "running" | "done" | "failed" | "stopped";

export function jobState(job: TerminalInfo): JobState {
  if (job.running) return "running";
  if (job.exit?.signal !== undefined) return "stopped";
  return job.exit?.code === 0 ? "done" : "failed";
}

/** How long a job has run, or ran: "45s", "12m", "2h 13m". */
export function jobDuration(job: TerminalInfo, now: number): string {
  const start = job.started_ms ?? job.created_ms;
  const end = job.running ? now : (job.exit?.ended_ms ?? now);
  return formatDuration(end - start);
}

export function formatDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d}d ${h % 24}h` : `${d}d`;
}

/** What to tell the user when a job ends. */
export function jobNote(job: TerminalInfo): string {
  const after = `after ${jobDuration(job, Date.now())}`;
  switch (jobState(job)) {
    case "done":
      return `Job ${job.title} finished ${after}.`;
    case "stopped":
      return `Job ${job.title} was stopped ${after}.`;
    default:
      return `Job ${job.title} failed (exit ${job.exit?.code ?? "?"}) ${after}.`;
  }
}
