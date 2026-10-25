import { createEffect, createMemo, createSignal, For, onCleanup, Show, untrack } from "solid-js";
import type { GitDiff } from "../backend/types";
import { type DiffLine, diffLines, hunkLabel, splitPath } from "../model/git";
import type { Workspace } from "../model/workspace";
import { ask, notify } from "./dialogs";
import type { DiffRequest } from "./GitPane";
import { FileIcon, Icon } from "./icons";

/** How often a working tree diff is read again while it shows. */
const POLL_MS = 3000;
/** Lines drawn before "Show all". */
const SHOWN_LINES = 4000;

/** A diff open as an editor tab. */
export interface DiffTab extends DiffRequest {
  kind: "diff";
  id: string;
}

export function diffTab(request: DiffRequest): DiffTab {
  const where = request.sha ?? (request.staged ? "staged" : "worktree");
  return { ...request, kind: "diff", id: `diff:${request.repo}:${where}:${request.file}` };
}

/** The tab's name: the file's, with what it compares. */
export function diffTitle(tab: DiffTab): string {
  const name = splitPath(tab.file).name;
  return tab.sha ? `${name} @ ${tab.sha.slice(0, 7)}` : tab.staged ? `${name} (staged)` : `${name} (changes)`;
}

type LinesOp = "stage_lines" | "unstage_lines" | "discard_lines";

/**
 * A file's diff in the editor area: old and new line numbers, removed lines
 * red and added ones green. A working tree diff is read again while it
 * shows, so it follows edits, and can be staged or unstaged from its bar:
 * the whole file, a hunk from its header, or the lines picked by their
 * numbers (click, Shift-click or drag), as `git add -p` would.
 */
export function DiffView(props: { ws: Workspace; tab: DiffTab; shown: boolean; onOpen: (path: string) => void; onStaged?: () => void }) {
  const [diff, setDiff] = createSignal<GitDiff>();
  const [error, setError] = createSignal<string>();
  const [all, setAll] = createSignal(false);
  const lines = createMemo(() => diffLines(diff()?.text ?? ""));
  /** Changed lines picked, by their places in the diff's text. */
  const [picked, setPicked] = createSignal<ReadonlySet<number>>(new Set());
  const pickable = () => !props.tab.sha && !!diff()?.lines;
  const changed = (line: DiffLine) => line.kind === "add" || line.kind === "del";
  const counts = createMemo(() => {
    let added = 0;
    let removed = 0;
    for (const line of lines()) {
      if (line.kind === "add") added++;
      else if (line.kind === "del") removed++;
    }
    return { added, removed };
  });

  let generation = 0;
  async function read() {
    const mine = ++generation;
    try {
      const next = await props.ws.call<GitDiff>("git_diff", {
        path: props.tab.repo,
        file: props.tab.file,
        staged: props.tab.staged,
        sha: props.tab.sha,
      });
      if (mine !== generation) return;
      if (next.text !== diff()?.text || next.binary !== diff()?.binary || next.lines !== diff()?.lines) {
        // Picks name places in the old text.
        setPicked(new Set<number>());
        setDiff(next);
      }
      setError(undefined);
    } catch (err) {
      if (mine === generation) setError(err instanceof Error ? err.message : String(err));
    }
  }

  createEffect(() => {
    if (props.shown) untrack(() => void read());
  });
  // A commit's diff never changes; the working tree's does.
  if (!props.tab.sha) {
    const poll = setInterval(() => props.shown && document.visibilityState === "visible" && void read(), POLL_MS);
    onCleanup(() => clearInterval(poll));
  }

  async function toggle() {
    try {
      await props.ws.call("git_run", {
        path: props.tab.repo,
        action: { op: props.tab.staged ? "unstage" : "stage", files: [props.tab.file] },
      });
      props.onStaged?.();
      await read();
    } catch (err) {
      notify(`Could not ${props.tab.staged ? "unstage" : "stage"} ${props.tab.file}: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  /** The changed lines of the hunk whose header is at `at`. */
  function hunkLines(at: number): number[] {
    const all = lines();
    const start = all.findIndex((line) => line.at === at);
    const out: number[] = [];
    for (let i = start + 1; i < all.length && all[i].kind !== "hunk"; i++) if (changed(all[i])) out.push(all[i].at);
    return out;
  }

  // Picking by the line numbers: a click picks or unpicks a line, a drag
  // does the same to every line it passes, Shift-click adds a range.
  let anchor: number | undefined;
  let drag: { on: boolean; base: ReadonlySet<number> } | undefined;
  function between(from: number, to: number): number[] {
    const [low, high] = from < to ? [from, to] : [to, from];
    return lines()
      .filter((line) => changed(line) && line.at >= low && line.at <= high)
      .map((line) => line.at);
  }
  function spread(base: ReadonlySet<number>, ats: number[], on: boolean) {
    const next = new Set(base);
    for (const at of ats) on ? next.add(at) : next.delete(at);
    setPicked(next);
  }
  function press(event: MouseEvent, line: DiffLine) {
    if (event.button !== 0) return;
    event.preventDefault();
    if (event.shiftKey && anchor !== undefined) {
      spread(picked(), between(anchor, line.at), true);
      return;
    }
    anchor = line.at;
    drag = { on: !picked().has(line.at), base: picked() };
    spread(drag.base, [line.at], drag.on);
  }
  function pass(line: DiffLine) {
    if (drag && anchor !== undefined) spread(drag.base, between(anchor, line.at), drag.on);
  }
  const release = () => (drag = undefined);
  window.addEventListener("mouseup", release);
  onCleanup(() => window.removeEventListener("mouseup", release));

  async function applyLines(op: LinesOp, ats: number[]) {
    const shown = diff();
    if (!shown || !ats.length) return;
    const name = splitPath(props.tab.file).name;
    if (op === "discard_lines") {
      const answer = await ask(
        `Throw away ${ats.length === 1 ? "this change" : "these changes"} to ${name}?`,
        ["Throw away", "Cancel"],
        "Staged changes stay. This can't be undone.",
      );
      if (answer === "Cancel") return;
    }
    try {
      await props.ws.call("git_run", {
        path: props.tab.repo,
        action: { op, file: props.tab.file, diff: shown.text, lines: [...ats].sort((a, b) => a - b) },
      });
      setPicked(new Set<number>());
      props.onStaged?.();
      await read();
    } catch (err) {
      const verb = op === "stage_lines" ? "stage" : op === "unstage_lines" ? "unstage" : "throw away";
      notify(`Could not ${verb} the lines of ${props.tab.file}: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  const pickedCount = () => picked().size;
  const linesWord = () => `${pickedCount()} line${pickedCount() === 1 ? "" : "s"}`;
  function onKey(event: KeyboardEvent) {
    if (event.key === "Escape" && pickedCount()) {
      event.stopPropagation();
      setPicked(new Set<number>());
    }
  }

  const where = () => (props.tab.sha ? `in commit ${props.tab.sha.slice(0, 7)}` : props.tab.staged ? "staged" : "not staged");
  const shownLines = () => (all() ? lines() : lines().slice(0, SHOWN_LINES));

  return (
    <div class="diffview" tabIndex={-1} onKeyDown={onKey}>
      <div class="kernel-bar diffbar">
        <FileIcon name={splitPath(props.tab.file).name} />
        <span class="dpath" title={`${props.tab.repo}/${props.tab.file}`}>
          {props.tab.file}
        </span>
        <span class="dwhere">{where()}</span>
        <span class="dcount">
          <Show when={counts().added}>
            <span class="dadd">+{counts().added}</span>
          </Show>
          <Show when={counts().removed}>
            <span class="ddel">−{counts().removed}</span>
          </Show>
        </span>
        <Show when={pickable() && !pickedCount()}>
          <span class="dhint">Click line numbers to pick lines</span>
        </Show>
        <span class="spacer" />
        <Show when={pickable() && pickedCount()}>
          <Show
            when={props.tab.staged}
            fallback={
              <>
                <button class="bar-button" onClick={() => void applyLines("stage_lines", [...picked()])}>
                  <Icon name="plus" small />
                  Stage {linesWord()}
                </button>
                <button class="bar-button" title="Throw away the picked lines' changes" onClick={() => void applyLines("discard_lines", [...picked()])}>
                  <Icon name="discard" small />
                  Throw away
                </button>
              </>
            }
          >
            <button class="bar-button" onClick={() => void applyLines("unstage_lines", [...picked()])}>
              <Icon name="minus" small />
              Unstage {linesWord()}
            </button>
          </Show>
          <button class="bar-button dclear" title="Pick no lines (Esc)" onClick={() => setPicked(new Set<number>())}>
            <Icon name="x" small />
          </button>
        </Show>
        <Show when={!props.tab.sha}>
          <button class="bar-button" onClick={toggle}>
            <Icon name={props.tab.staged ? "minus" : "plus"} small />
            {props.tab.staged ? "Unstage" : "Stage"}
          </button>
        </Show>
        <button class="bar-button" title="Open the file in the editor" onClick={() => props.onOpen(`${props.tab.repo}/${props.tab.file}`)}>
          <Icon name="open" small />
          Open
        </button>
      </div>
      <div class="dbody">
        <Show when={error()}>
          <p class="vempty err">{error()}</p>
        </Show>
        <Show when={diff()?.binary}>
          <p class="vempty">A binary file: there are no lines to compare.</p>
        </Show>
        <Show when={diff() && !diff()!.binary && !lines().length}>
          <p class="vempty">{props.tab.staged ? "Nothing of this file is staged now." : "No changes left in this file."}</p>
        </Show>
        <div class="dlines">
          <For each={shownLines()}>
            {(line) => (
              <Show
                when={line.kind === "hunk"}
                fallback={
                  <div
                    class={`dl ${line.kind}`}
                    classList={{ pickable: pickable() && changed(line), picked: picked().has(line.at) }}
                    onMouseDown={(event) => pickable() && changed(line) && (event.target as HTMLElement).closest(".dn, .dm") && press(event, line)}
                    onMouseEnter={() => pickable() && changed(line) && pass(line)}
                  >
                    <span class="dn">{line.old ?? ""}</span>
                    <span class="dn">{line.new ?? ""}</span>
                    <span class="dm">{line.kind === "add" ? "+" : line.kind === "del" ? "−" : ""}</span>
                    <span class="dt">{line.text}</span>
                  </div>
                }
              >
                <div class="dl hunk" classList={{ acts: pickable() }} title={line.text}>
                  <span class="dh">{hunkLabel(line.text).lines}</span>
                  <Show when={pickable()}>
                    <span class="dhacts">
                      <Show
                        when={props.tab.staged}
                        fallback={
                          <>
                            <button title="Stage this hunk" onClick={() => void applyLines("stage_lines", hunkLines(line.at))}>
                              Stage
                            </button>
                            <button title="Throw away this hunk's changes" onClick={() => void applyLines("discard_lines", hunkLines(line.at))}>
                              Throw away
                            </button>
                          </>
                        }
                      >
                        <button title="Unstage this hunk" onClick={() => void applyLines("unstage_lines", hunkLines(line.at))}>
                          Unstage
                        </button>
                      </Show>
                    </span>
                  </Show>
                  <span class="dt">{hunkLabel(line.text).context}</span>
                </div>
              </Show>
            )}
          </For>
        </div>
        <Show when={!all() && lines().length > SHOWN_LINES}>
          <button class="gmore" onClick={() => setAll(true)}>
            Show all {lines().length.toLocaleString()} lines
          </button>
        </Show>
        <Show when={diff()?.truncated}>
          <p class="vempty">The diff is cut short at 2 MB.</p>
        </Show>
      </div>
    </div>
  );
}
