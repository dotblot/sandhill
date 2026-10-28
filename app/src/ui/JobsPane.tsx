import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import type { JobUsage, SystemStats, TerminalInfo } from "../backend/types";
import { jobDuration, jobState } from "../model/jobs";
import { formatBytes } from "../model/machine";
import type { TerminalStore } from "../model/terminals";
import type { Workspace } from "../model/workspace";
import { ask, askText, notify } from "./dialogs";
import { Icon } from "./icons";
import { TermView } from "./termView";

/** How often a running job's CPU and memory are asked for while the tab shows. */
const POLL_MS = 3000;

export interface JobsApi {
  /** Shows a job's log. */
  show(id: string): void;
}

/**
 * The Console's Jobs tab: scripts and pipelines running in the background on
 * the daemon's machine, newest first, each with how long it has run, its CPU
 * and memory, and its log. A job keeps running when the window closes.
 */
export function JobsPane(props: {
  ws: Workspace;
  store: TerminalStore;
  shown: boolean;
  openLink: (url: string) => void;
  api?: (api: JobsApi) => void;
}) {
  const store = props.store;
  const jobs = () => [...store.jobs()].reverse();
  const [picked, setPicked] = createSignal<string>();
  const selected = () => jobs().find((j) => j.id === picked()) ?? jobs()[0];
  const selectedId = createMemo(() => selected()?.id);
  const running = createMemo(() => store.jobs().some((j) => j.running));
  const [usage, setUsage] = createSignal(new Map<string, JobUsage>());
  const [now, setNow] = createSignal(Date.now());
  const views = new Map<string, TermView>();
  let host!: HTMLDivElement;

  // Each job's log keeps its own view, so switching keeps their scrollback.
  createEffect(
    on([selectedId, () => props.shown], ([id, shown]) => {
      if (!shown || !id) return;
      let view = views.get(id);
      if (!view) {
        view = new TermView(store, props.openLink);
        views.set(id, view);
      }
      for (const [other, v] of views) v.el.hidden = other !== id;
      const showing = view;
      void showing.mount(host).then(() => showing.bind(id));
    }),
  );
  // Views of jobs that were removed go.
  createEffect(() => {
    const ids = new Set(store.jobs().map((j) => j.id));
    for (const [id, view] of views) {
      if (ids.has(id)) continue;
      view.dispose();
      views.delete(id);
    }
  });
  onCleanup(() => {
    for (const view of views.values()) view.dispose();
  });

  // CPU, memory and run time, while the tab shows and a job runs.
  createEffect(() => {
    if (!props.shown || !running()) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const stats = await props.ws.call<SystemStats>("system_stats", {});
        setUsage(new Map((stats.jobs ?? []).map((j) => [j.terminal, j])));
      } catch {
        // A dropped link, or a daemon too old to say; the next poll tries again.
      }
      if (!stopped) timer = setTimeout(() => void poll(), POLL_MS);
    };
    const tick = setInterval(() => setNow(Date.now()), 1000);
    void poll();
    onCleanup(() => {
      stopped = true;
      clearTimeout(timer);
      clearInterval(tick);
    });
  });

  props.api?.({ show: (id) => setPicked(id) });

  async function newJob() {
    const command = await askText(
      "Run a command in the background",
      "",
      "Run",
      "It runs in this project's folder with your shell's setup, as in the Terminal, and keeps running when Sandhill closes.",
    );
    if (!command?.trim()) return;
    try {
      setPicked((await store.startJob({ command })).id);
    } catch (err) {
      notify(`Could not start the job: ${message(err)}`);
    }
  }

  async function act(job: TerminalInfo, what: "stop" | "again" | "remove") {
    try {
      if (what === "stop") {
        const choice = await ask(`Stop ${job.title}?`, ["Stop", "Cancel"], "Its log stays, and it can run again.");
        if (choice === "Stop") await store.stop(job.id);
      } else if (what === "again") {
        await store.restart(job.id);
        setPicked(job.id);
      } else {
        await store.close(job.id);
      }
    } catch (err) {
      notify(message(err));
    }
  }

  const detail = (job: TerminalInfo) => {
    const parts = [jobDuration(job, now())];
    const use = job.running ? usage().get(job.id) : undefined;
    if (use?.cpu !== undefined) parts.push(`${Math.round(use.cpu)}% CPU`);
    if (use) parts.push(formatBytes(use.rss));
    return parts.join(" · ");
  };

  return (
    <div class="jobs" hidden={!props.shown}>
      <div class="jobs-list" role="list">
        <div class="jobs-head">
          <button class="jobs-new" title="Run a command in the background" onClick={() => void newJob()}>
            <Icon name="plus" small /> New job
          </button>
        </div>
        <For each={jobs()}>
          {(job) => (
            <div
              class="job"
              role="listitem"
              classList={{ on: selected()?.id === job.id }}
              title={job.job?.path ?? job.job?.command ?? job.title}
              onClick={() => setPicked(job.id)}
            >
              <JobMark job={job} />
              <span class="job-name">{job.name ?? job.title}</span>
              <span class="job-detail">{detail(job)}</span>
              <span class="job-acts">
                <Show
                  when={job.running}
                  fallback={
                    <>
                      <button title="Run again" onClick={(e) => (e.stopPropagation(), void act(job, "again"))}>
                        <Icon name="refresh" small />
                      </button>
                      <button title="Remove, with its log" onClick={(e) => (e.stopPropagation(), void act(job, "remove"))}>
                        <Icon name="trash" small />
                      </button>
                    </>
                  }
                >
                  <button title="Stop" onClick={(e) => (e.stopPropagation(), void act(job, "stop"))}>
                    <Icon name="stop" small />
                  </button>
                </Show>
              </span>
            </div>
          )}
        </For>
        <Show when={!jobs().length}>
          <p class="jobs-empty">
            No jobs yet. Run a script in the background with Run as job in the file tree's menu, or start a command with New
            job. Jobs keep running when Sandhill closes.
          </p>
        </Show>
      </div>
      <div class="jobs-log">
        <Show when={selected()}>
          {(job) => (
            <div class="jobs-line">
              <span class="job-state">{stateLabel(job())}</span>
              <span class="job-where">{whatRuns(job())}</span>
              <span class="job-where">in {job().cwd === job().project ? job().cwd.split("/").pop() : inProject(job(), job().cwd)}</span>
            </div>
          )}
        </Show>
        <div class="term-host" ref={host} />
      </div>
    </div>
  );
}

function JobMark(props: { job: TerminalInfo }) {
  return (
    <span class="job-mark" data-state={jobState(props.job)}>
      <Show when={jobState(props.job) === "running"} fallback={<Icon name={jobState(props.job) === "done" ? "check" : "x"} small />}>
        <span class="spin" />
      </Show>
    </span>
  );
}

/** The script and its arguments, or the command line. */
function whatRuns(job: TerminalInfo): string {
  const { path, command } = job.job ?? {};
  if (path) return command ? `${inProject(job, path)} ${command}` : inProject(job, path);
  return command ?? job.title;
}

/** A path inside the job's project, from the project's folder. */
function inProject(job: TerminalInfo, path: string): string {
  return job.project && path.startsWith(`${job.project}/`) ? path.slice(job.project.length + 1) : path;
}

function stateLabel(job: TerminalInfo): string {
  switch (jobState(job)) {
    case "running":
      return "Running";
    case "done":
      return "Finished";
    case "stopped":
      return "Stopped";
    default:
      return `Failed, exit ${job.exit?.code ?? "?"}`;
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
