import { createEffect, createMemo, createSignal, For, onCleanup, Show, untrack } from "solid-js";
import type { GitDiff } from "../backend/types";
import { diffLines, hunkLabel, splitPath } from "../model/git";
import type { Workspace } from "../model/workspace";
import { notify } from "./dialogs";
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

/**
 * A file's diff in the editor area: old and new line numbers, removed lines
 * red and added ones green. A working tree diff is read again while it
 * shows, so it follows edits, and can be staged or unstaged from its bar.
 */
export function DiffView(props: { ws: Workspace; tab: DiffTab; shown: boolean; onOpen: (path: string) => void; onStaged?: () => void }) {
  const [diff, setDiff] = createSignal<GitDiff>();
  const [error, setError] = createSignal<string>();
  const [all, setAll] = createSignal(false);
  const lines = createMemo(() => diffLines(diff()?.text ?? ""));
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
      if (next.text !== diff()?.text || next.binary !== diff()?.binary) setDiff(next);
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

  const where = () => (props.tab.sha ? `in commit ${props.tab.sha.slice(0, 7)}` : props.tab.staged ? "staged" : "not staged");
  const shownLines = () => (all() ? lines() : lines().slice(0, SHOWN_LINES));

  return (
    <div class="diffview">
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
        <span class="spacer" />
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
                  <div class={`dl ${line.kind}`}>
                    <span class="dn">{line.old ?? ""}</span>
                    <span class="dn">{line.new ?? ""}</span>
                    <span class="dm">{line.kind === "add" ? "+" : line.kind === "del" ? "−" : ""}</span>
                    <span class="dt">{line.text}</span>
                  </div>
                }
              >
                <div class="dl hunk" title={line.text}>
                  <span class="dh">{hunkLabel(line.text).lines}</span>
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
