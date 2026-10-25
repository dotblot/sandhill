import { createEffect, createMemo, createSignal, For, type JSX, onCleanup, Show, untrack } from "solid-js";
import type { FileText, GitAction, GitCommit, GitCommitInfo, GitFile, GitLog, GitMessage, GitRef, GitStatus } from "../backend/types";
import {
  avatar,
  BIG_FILE,
  branchName,
  commitMessage,
  fileMark,
  fileSize,
  LANE_COLORS,
  layoutGraph,
  lineCounts,
  splitMessage,
  splitPath,
  stagedFiles,
  timeAgo,
  unstagedFiles,
  WORKTREE,
} from "../model/git";
import type { Workspace } from "../model/workspace";
import { ask, askText, notify } from "./dialogs";
import { FileIcon, Icon, type IconName } from "./icons";
import { closeOnOutside } from "./popup";

/** How often the status is read while the pane shows. */
const POLL_MS = 3000;
/** Commits read at a time. */
const PAGE = 200;
/** The graph's row height and lane spacing, in pixels. */
const ROW = 22;
const LANE = 16;
const MAX_LANES = 8;
/** The Branch / Tag column. */
const REFS_WIDTH = 72;

/** A diff to open as a tab: a working tree file's (staged or not) or one in a commit. */
export interface DiffRequest {
  repo: string;
  file: string;
  staged: boolean;
  sha?: string;
}

/** A ref as a badge: a local branch also on its remote shows the globe, once. */
interface Badge {
  kind: GitRef["kind"];
  name: string;
  remote?: boolean;
}

function badges(refs: GitRef[] = []): Badge[] {
  const local = new Set(refs.filter((r) => r.kind === "head" || r.kind === "branch").map((r) => r.name));
  const out: Badge[] = [];
  for (const ref of refs) {
    if (ref.kind === "remote") {
      const name = ref.name.slice(ref.name.indexOf("/") + 1);
      if (local.has(name)) continue;
      out.push({ kind: "remote", name: ref.name });
    } else {
      const remote = ref.kind !== "tag" && refs.some((r) => r.kind === "remote" && r.name.endsWith(`/${ref.name}`));
      out.push({ kind: ref.kind, name: ref.name, remote });
    }
  }
  return out;
}

let instances = 0;

/**
 * The Git pane: the project's branch with fetch, pull and push; the commit
 * graph of every branch, with author avatars (people, Claude Code, Codex);
 * then, for the working tree, staged and unstaged files and a commit box
 * whose Write it asks an agent for the message, or for a picked commit,
 * its message and files. A file opens its diff as a tab. Git runs on the
 * project's machine, through the daemon.
 */
export function GitPane(props: {
  ws: Workspace;
  project: string;
  shown: boolean;
  refreshKey?: number;
  onDiff: (request: DiffRequest) => void;
  /** Opens a file of the repository in the editor, by absolute path. */
  onOpen: (path: string) => void;
}) {
  const ws = props.ws;
  const gradient = `gav${++instances}`;
  const [status, setStatus] = createSignal<GitStatus>();
  const [error, setError] = createSignal<string>();
  const [commits, setCommits] = createSignal<GitCommit[]>([]);
  const [more, setMore] = createSignal(false);
  const [picked, setPicked] = createSignal<string>(WORKTREE);
  const [info, setInfo] = createSignal<GitCommitInfo>();
  const [busy, setBusy] = createSignal<string>();
  const [writing, setWriting] = createSignal(false);
  const [summary, setSummary] = createSignal("");
  const [body, setBody] = createSignal("");
  const [amend, setAmend] = createSignal(false);
  const [menu, setMenu] = createSignal<{ kind: "branches" | "merge" | "stash" | "more" | "commit"; anchor: HTMLElement }>();
  const [userName, setUserName] = createSignal("");
  const [userEmail, setUserEmail] = createSignal("");
  const [now, setNow] = createSignal(Date.now());

  let latest = 0;
  let inFlight = 0;
  /** The refs the log was read for. */
  let logRefs: string | undefined;
  /** The merge message put in the commit boxes. */
  let mergeFilled: string | undefined;
  async function readStatus() {
    const mine = ++latest;
    inFlight++;
    try {
      const next = await ws.call<GitStatus>("git_status", { path: props.project });
      if (mine !== latest) return;
      // Polling mostly reads the same again; only a change redraws.
      if (JSON.stringify(next) !== JSON.stringify(status())) setStatus(next);
      fillMergeMessage(next.merge_message);
      setError(undefined);
      setNow(Date.now());
      if (next.repo && next.refs !== logRefs) void readLog(next.refs);
      if (!next.repo) setCommits([]);
    } catch (err) {
      if (mine === latest) setError(message(err));
    } finally {
      inFlight--;
    }
  }

  /** A merge's commit starts with git's message; it goes again with the merge. */
  function fillMergeMessage(text?: string) {
    if (text === mergeFilled) return;
    const split = splitMessage(text ?? mergeFilled ?? "");
    if (text && !summary().trim() && !body().trim()) {
      setSummary(split.summary);
      setBody(split.body);
    } else if (!text && summary() === split.summary && body() === split.body) {
      setSummary("");
      setBody("");
    }
    mergeFilled = text;
  }

  let logGeneration = 0;
  async function readLog(refs: string | undefined, append = false) {
    const mine = ++logGeneration;
    const skip = append ? commits().length : 0;
    try {
      const log = await ws.call<GitLog>("git_log", { path: props.project, skip, limit: PAGE });
      if (mine !== logGeneration) return;
      logRefs = refs;
      setCommits(append ? [...commits(), ...log.commits] : log.commits);
      setMore(log.more);
      const sha = picked();
      if (sha !== WORKTREE && !log.commits.some((c) => c.sha === sha) && !append) setPicked(WORKTREE);
    } catch (err) {
      if (mine === logGeneration) setError(message(err));
    }
  }

  createEffect(() => {
    props.refreshKey;
    if (!props.shown) return;
    untrack(() => {
      logRefs = undefined;
      void readStatus();
    });
  });
  const poll = setInterval(() => {
    if (props.shown && !inFlight && document.visibilityState === "visible") void readStatus();
  }, POLL_MS);
  const focus = () => props.shown && void readStatus();
  window.addEventListener("focus", focus);
  onCleanup(() => {
    clearInterval(poll);
    window.removeEventListener("focus", focus);
  });

  // A picked commit's message and files.
  createEffect(() => {
    const sha = picked();
    setInfo(undefined);
    if (sha === WORKTREE) return;
    ws.call<GitCommitInfo>("git_show", { path: props.project, sha })
      .then((shown) => picked() === sha && setInfo(shown))
      .catch((err) => notify(`Could not read the commit: ${message(err)}`));
  });

  const repo = () => status()?.repo ?? "";
  const staged = () => (status() ? stagedFiles(status()!) : []);
  const unstaged = () => (status() ? unstagedFiles(status()!) : []);
  const needsUser = () => !!status()?.repo && (!status()!.user_name || !status()!.user_email);
  /** A commit needs something staged, unless it amends or ends a merge. */
  const canCommit = () => amend() || staged().length > 0 || status()?.state === "merging";

  /** Runs a git action; reads the status again after, whatever happened. */
  async function act(action: GitAction, failed: string, done?: (output: string) => string | undefined): Promise<boolean> {
    setBusy(action.op);
    try {
      const result = await ws.call<{ output: string }>("git_run", { path: props.project, action });
      const said = done?.(result.output.trim());
      if (said) notify(said, "info");
      return true;
    } catch (err) {
      notify(`${failed}: ${message(err)}`);
      return false;
    } finally {
      setBusy(undefined);
      await readStatus();
    }
  }
  const lastLine = (output: string) => output.split("\n").filter(Boolean).pop();

  /** Asks before staging a file too big for GitHub; "Ignore it" adds it to .gitignore instead. */
  async function stage(files: GitFile[], all = false) {
    const big = files.filter((f) => (f.size ?? 0) >= BIG_FILE);
    const skip = new Set<string>();
    for (const file of big) {
      const answer = await ask(
        `Stage ${splitPath(file.path).name}? It is ${fileSize(file.size!)}.`,
        ["Ignore it", "Stage anyway", "Cancel"],
        `${tooBig(file.size!)} Data this big usually stays out of Git: Ignore it adds the file to .gitignore, so Git leaves it out.`,
      );
      if (answer === "Cancel") return;
      if (answer === "Ignore it") {
        if (!(await ignore(file.path))) return;
        skip.add(file.path);
      }
    }
    const paths = files.map((f) => f.path).filter((p) => !skip.has(p));
    if (skip.size && !paths.includes(".gitignore")) paths.push(".gitignore");
    if (!paths.length) return void readStatus();
    await act({ op: "stage", files: all && !big.length ? [] : paths }, "Could not stage");
  }

  async function ignore(path: string): Promise<boolean> {
    const file = `${repo()}/.gitignore`;
    let text = "";
    try {
      text = (await ws.call<FileText>("read_file", { path: file })).text;
    } catch {
      // No .gitignore yet.
    }
    const line = `/${path}`;
    if (!text.split("\n").includes(line)) {
      try {
        await ws.call("write_file", { path: file, text: `${text}${text && !text.endsWith("\n") ? "\n" : ""}${line}\n` });
      } catch (err) {
        notify(`Could not add ${path} to .gitignore: ${message(err)}`);
        return false;
      }
    }
    return true;
  }

  async function discard(files: GitFile[]) {
    const fresh = files.every((f) => f.unstaged === "?");
    const name = files.length === 1 ? splitPath(files[0].path).name : `${files.length} files`;
    const answer = await ask(
      fresh ? `Delete ${name}?` : `Throw away the changes to ${name}?`,
      [fresh ? "Delete" : "Throw away", "Cancel"],
      fresh ? "It isn't in Git, so it can't be brought back." : "Staged changes stay. This can't be undone.",
    );
    if (answer === "Cancel") return;
    await act({ op: "discard", files: files.map((f) => f.path) }, "Could not throw the changes away");
  }

  async function commit(then?: "push" | "all") {
    const text = commitMessage(summary(), body());
    if (!text) return;
    if (then === "all" && unstaged().length) {
      await stage(unstaged(), true);
      if (!staged().length) return;
    }
    if (!canCommit()) return;
    const ok = await act({ op: "commit", message: text, amend: amend() }, amend() ? "Could not change the last commit" : "Could not commit");
    if (!ok) return;
    setSummary("");
    setBody("");
    setAmend(false);
    if (then === "push") await push();
  }

  async function toggleAmend() {
    const on = !amend();
    setAmend(on);
    if (!on || summary().trim() || !status()?.head) return;
    try {
      const last = await ws.call<GitCommitInfo>("git_show", { path: props.project, sha: status()!.head });
      const split = splitMessage(last.message);
      setSummary(split.summary);
      setBody(split.body);
    } catch {
      // The boxes stay empty.
    }
  }

  async function writeIt() {
    setWriting(true);
    try {
      const written = await ws.call<GitMessage>("git_write_message", { path: props.project });
      setSummary(written.summary);
      setBody(written.body);
    } catch (err) {
      notify(`Could not write a message: ${message(err)}`);
    } finally {
      setWriting(false);
    }
  }

  const push = () =>
    act({ op: "push" }, "Could not push", () => {
      const s = status();
      return `Pushed ${s?.branch ?? "the branch"}${s?.upstream ? ` to ${s.upstream.split("/")[0]}` : ""}`;
    });

  async function undo() {
    const s = status();
    const head = commits().find((c) => c.sha === s?.head);
    const pushed = !!s?.upstream && s.ahead === 0;
    const answer = await ask(
      head ? `Undo “${head.subject}”?` : "Undo the last commit?",
      ["Undo", "Cancel"],
      pushed
        ? `It's already pushed to ${s!.upstream}; undoing it here doesn't take it back there. Its changes stay staged.`
        : "Its changes stay staged, so you can commit them again.",
    );
    if (answer === "Undo") await act({ op: "undo_commit" }, "Could not undo the commit");
  }

  /** Merges `branch` into the one checked out, after asking. */
  async function merge(branch: string) {
    setMenu(undefined);
    const into = status()?.branch ?? "HEAD";
    const answer = await ask(
      `Merge ${branch} into ${into}?`,
      ["Merge", "Cancel"],
      `Its commits join ${into}. Where both changed the same lines, the files are marked ! to settle: edit them, or keep Mine or take Theirs, then commit.`,
    );
    if (answer !== "Merge") return;
    await act({ op: "merge", branch }, `Could not merge ${branch}`, (output) =>
      /CONFLICT/.test(output) ? undefined : /up to date/i.test(output) ? `${into} already has everything in ${branch}` : `Merged ${branch} into ${into}`,
    );
  }

  async function newBranch() {
    setMenu(undefined);
    const text = await askText("New branch", "", "Create", "Made from the current commit, and switched to.");
    if (text === undefined) return;
    const name = branchName(text);
    if (!name) return notify(`“${text}” can't be a branch name: use letters, numbers, - _ / and no spaces.`);
    await act({ op: "create_branch", name }, "Could not make the branch");
  }

  async function stash() {
    setMenu(undefined);
    const text = await askText("Stash changes", "", "Stash", "Puts every change aside, new files too, and leaves the working tree clean.");
    if (text === undefined) return;
    await act({ op: "stash", message: text.trim() || undefined }, "Could not stash");
  }

  // The graph, with the working tree on top as a commit whose parent is HEAD.
  const rows = createMemo(() => {
    const list = commits();
    const head = status()?.head;
    const worktree = { sha: WORKTREE, parents: head ? [head] : [] } as GitCommit;
    return [worktree, ...list];
  });
  const graph = createMemo(() => layoutGraph(rows()));
  const graphWidth = () => Math.max(40, 6 + Math.min(graph().lanes, MAX_LANES) * LANE);
  const x = (lane: number) => 12 + lane * LANE;
  const y = (row: number) => row * ROW + ROW / 2;
  const colors = createMemo(() => {
    const pairs = new Map<string, [string, string]>();
    for (const c of commits()) {
      const a = avatar(c);
      pairs.set(a.colors.join(), a.colors);
    }
    return [...pairs.values()];
  });
  const gradientId = (pair: [string, string]) => `${gradient}-${colors().findIndex((p) => p.join() === pair.join())}`;

  function edgePath(row: number, from: number, to: number) {
    const [x1, y1, x2, y2] = [x(from), y(row), x(to), y(row + 1)];
    if (x1 === x2) return `M${x1} ${y1}V${y2}`;
    return `M${x1} ${y1}C${x1} ${y1 + 12} ${x2} ${y2 - 12} ${x2} ${y2}`;
  }

  function onScroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    if (more() && el.scrollTop + el.clientHeight > el.scrollHeight - 200) {
      setMore(false);
      void readLog(logRefs, true);
    }
  }

  const branchLabel = () => {
    const s = status();
    if (!s) return "";
    return s.branch ?? (s.head ? `HEAD at ${s.head.slice(0, 7)}` : "");
  };


  return (
    <div class="gitpane">
      <Show when={error() && !status()}>
        <p class="vempty err">{error()}</p>
      </Show>
      <Show when={status() && !status()!.repo}>
        <div class="gnone">
          <p>This project isn't a Git repository yet.</p>
          <button class="vbtn primary" disabled={!!busy()} onClick={() => act({ op: "init" }, "Could not start a repository")}>
            Start one here
          </button>
        </div>
      </Show>
      <Show when={status()?.repo}>
        <div class="ghead">
          <Icon name="branch" small />
          <button
            class="gbranch"
            title={status()!.upstream ? `${branchLabel()}, following ${status()!.upstream}` : `${branchLabel()}; switch branch`}
            onClick={(e) => setMenu({ kind: "branches", anchor: e.currentTarget })}
          >
            {branchLabel()}
          </button>
          <Show when={status()!.ahead || status()!.behind}>
            <span class="ahead" title={`${status()!.ahead} to push, ${status()!.behind} to pull`}>
              {[status()!.ahead ? `↑${status()!.ahead}` : "", status()!.behind ? `↓${status()!.behind}` : ""].filter(Boolean).join(" ")}
            </span>
          </Show>
          <span class="spacer" />
          <HeadButton
            icon="fetch"
            title="Fetch: see what's new on the remote"
            busy={busy()}
            op="fetch"
            disabled={!status()!.remotes.length}
            run={() => act({ op: "fetch" }, "Could not fetch")}
          />
          <HeadButton
            icon="pull"
            title={status()!.behind ? `Pull ${status()!.behind} commit${status()!.behind === 1 ? "" : "s"}` : "Pull"}
            busy={busy()}
            op="pull"
            disabled={!status()!.upstream}
            count={status()!.behind}
            run={() => act({ op: "pull" }, "Could not pull", lastLine)}
          />
          <HeadButton
            icon="push"
            title={status()!.ahead ? `Push ${status()!.ahead} commit${status()!.ahead === 1 ? "" : "s"}` : "Push"}
            busy={busy()}
            op="push"
            disabled={!status()!.head}
            count={status()!.ahead}
            run={push}
          />
          <HeadButton icon="undo" title="Undo the last commit" busy={busy()} op="undo_commit" disabled={!status()!.head} run={undo} />
          <button class="ib" title="Branches" aria-label="Branches" onClick={(e) => setMenu({ kind: "branches", anchor: e.currentTarget })}>
            <Icon name="branch" small />
          </button>
          <button class="ib" title="Stash" aria-label="Stash" onClick={(e) => setMenu({ kind: "stash", anchor: e.currentTarget })}>
            <Icon name="stash" small />
            <Show when={status()!.stashes}>
              <span class="n">{status()!.stashes}</span>
            </Show>
          </button>
          <button class="ib" title="More" aria-label="More" onClick={(e) => setMenu({ kind: "more", anchor: e.currentTarget })}>
            <Icon name="more" small />
          </button>
        </div>
        <div class="gcols" style={{ "grid-template-columns": `${REFS_WIDTH}px ${graphWidth()}px minmax(0, 1fr)` }}>
          <span>Branch / Tag</span>
          <span>Graph</span>
          <span>Message</span>
        </div>
        <div class="glog" onScroll={onScroll}>
          <div class="ggwrap" style={{ left: `${8 + REFS_WIDTH}px` }}>
            <svg class="ggraph" width={graphWidth()} height={rows().length * ROW} aria-hidden="true">
              <defs>
                <For each={colors()}>
                  {(pair) => (
                    <linearGradient id={gradientId(pair)} x1="0" y1="0" x2="1" y2="1">
                      <stop offset="0" stop-color={pair[0]} />
                      <stop offset="1" stop-color={pair[1]} />
                    </linearGradient>
                  )}
                </For>
              </defs>
              <For each={graph().rows}>
                {(row, i) => (
                  <For each={row.edges}>
                    {(edge) => (
                      <path
                        d={edgePath(i(), edge.from, edge.to)}
                        fill="none"
                        stroke={LANE_COLORS[edge.color % LANE_COLORS.length]}
                        stroke-width="2"
                        stroke-dasharray={i() === 0 ? "2 2" : undefined}
                      />
                    )}
                  </For>
                )}
              </For>
              <For each={rows()}>
                {(commit, i) => {
                  const lane = () => graph().rows[i()]?.lane ?? 0;
                  const color = () => LANE_COLORS[lane() % LANE_COLORS.length];
                  if (commit.sha === WORKTREE) {
                    return <circle cx={x(lane())} cy={y(i())} r="4.5" fill="var(--bg)" stroke={color()} stroke-width="1.5" stroke-dasharray="2 1.6" />;
                  }
                  const face = avatar(commit);
                  return (
                    <g>
                      <Show when={commit.sha === status()?.head}>
                        <circle cx={x(lane())} cy={y(i())} r="10" fill="none" stroke={color()} stroke-width="1" stroke-dasharray="2 1.6" />
                      </Show>
                      <circle cx={x(lane())} cy={y(i())} r="8" fill={`url(#${gradientId(face.colors)})`} stroke={color()} stroke-width="1.5" />
                      <text x={x(lane())} y={y(i()) + 2.4} text-anchor="middle" font-size="6.8" font-weight="700" fill="#fff">
                        {face.initials}
                      </text>
                    </g>
                  );
                }}
              </For>
            </svg>
          </div>
          <For each={rows()}>
            {(commit, i) => {
              const lane = () => graph().rows[i()]?.lane ?? 0;
              const worktree = commit.sha === WORKTREE;
              const face = worktree ? undefined : avatar(commit);
              const title = () =>
                worktree
                  ? "The files as they are now"
                  : `${commit.subject}\n${face!.who}, ${new Date(commit.time_ms).toLocaleString()}\n${commit.sha.slice(0, 10)}`;
              return (
                <div
                  class="grow"
                  classList={{ on: picked() === commit.sha }}
                  style={{ "grid-template-columns": `${REFS_WIDTH}px ${graphWidth()}px minmax(0, 1fr)` }}
                  title={title()}
                  onClick={() => setPicked(commit.sha)}
                >
                  <div class="gcol-ref">
                    <For each={badges(commit.refs).slice(0, 1)}>{(badge) => <RefBadge badge={badge} />}</For>
                    <Show when={badges(commit.refs).length > 1}>
                      <span class="gref more" title={badges(commit.refs).slice(1).map((b) => b.name).join("\n")}>
                        +{badges(commit.refs).length - 1}
                      </span>
                    </Show>
                  </div>
                  <div
                    class="gcol-msg"
                    classList={{ wt: worktree }}
                    style={{ "--lane": worktree ? "transparent" : LANE_COLORS[lane() % LANE_COLORS.length] }}
                  >
                    <Show when={worktree} fallback={<span class="gsubj">{commit.subject}</span>}>
                      <span class="gsubj">Working tree</span>
                      <Show when={staged().length}>
                        <span class="gbadge ok">{staged().length} staged</span>
                      </Show>
                      <Show when={unstaged().length}>
                        <span class="gbadge warn">{unstaged().length} unstaged</span>
                      </Show>
                      <Show when={!staged().length && !unstaged().length}>
                        <span class="gbadge">clean</span>
                      </Show>
                    </Show>
                    <Show when={!worktree}>
                      <span class="gwhen">{timeAgo(commit.time_ms, now())}</span>
                    </Show>
                  </div>
                </div>
              );
            }}
          </For>
          <Show when={more()}>
            <button class="gmore" onClick={() => void readLog(logRefs, true)}>
              Older commits
            </button>
          </Show>
        </div>
        <Show
          when={picked() === WORKTREE}
          fallback={<CommitDetail info={info()} onClose={() => setPicked(WORKTREE)} onDiff={(file) => props.onDiff({ repo: repo(), file, staged: false, sha: picked() })} />}
        >
          <div class="gstage">
            <Show when={status()!.state}>
              <div class="gstate">
                <span>
                  {status()!.state === "merging" ? "Merging" : status()!.state === "rebasing" ? "Rebasing" : status()!.state === "cherry-picking" ? "Cherry-picking" : "Reverting"}
                  {status()!.state === "merging"
                    ? ": fix each ! file in the editor and stage it, or pick Mine or Theirs; then commit."
                    : ": fix the conflicts, stage them, then commit."}
                </span>
                <button class="vbtn" disabled={!!busy()} onClick={() => act({ op: "abort" }, "Could not stop it")}>
                  Stop
                </button>
              </div>
            </Show>
            <div class="gfiles">
              <Show when={staged().length}>
                <FileGroup
                  title="Staged"
                  dot="idle"
                  files={staged()}
                  staged
                  busy={!!busy()}
                  onAll={() => act({ op: "unstage", files: [] }, "Could not unstage")}
                  onToggle={(f) => act({ op: "unstage", files: [f.path] }, "Could not unstage")}
                  onDiff={(f) => props.onDiff({ repo: repo(), file: f.path, staged: true })}
                  onOpen={(f) => props.onOpen(`${repo()}/${f.path}`)}
                />
              </Show>
              <Show when={unstaged().length}>
                <FileGroup
                  title="Unstaged"
                  dot="wait"
                  files={unstaged()}
                  busy={!!busy()}
                  onAll={() => stage(unstaged(), true)}
                  onToggle={(f) => stage([f])}
                  onDiscard={(f) => discard([f])}
                  onResolve={
                    status()!.state === "merging"
                      ? (f, side) => act({ op: "resolve", file: f.path, side }, `Could not settle ${f.path}`)
                      : undefined
                  }
                  onDiff={(f) => props.onDiff({ repo: repo(), file: f.path, staged: false })}
                  onOpen={(f) => props.onOpen(`${repo()}/${f.path}`)}
                />
              </Show>
              <Show when={status()!.more}>
                <div class="var-note">Only the first 2,000 changed files show.</div>
              </Show>
              <Show when={!staged().length && !unstaged().length}>
                <div class="var-note">No changes since the last commit.</div>
              </Show>
            </div>
            <Show when={needsUser()}>
              <div class="guser">
                <span>Commits need your name and email:</span>
                <div class="pi-row">
                  <input class="vfilter" placeholder="Name" aria-label="Your name" value={userName()} onInput={(e) => setUserName(e.currentTarget.value)} />
                  <input class="vfilter" placeholder="Email" aria-label="Your email" value={userEmail()} onInput={(e) => setUserEmail(e.currentTarget.value)} />
                  <button
                    class="vbtn primary"
                    disabled={!userName().trim() || !userEmail().includes("@") || !!busy()}
                    onClick={() => act({ op: "set_user", name: userName(), email: userEmail() }, "Could not save your name and email")}
                  >
                    Save
                  </button>
                </div>
              </div>
            </Show>
            <div class="gmsg">
              <input
                placeholder={amend() ? "Summary of the last commit" : "Summary"}
                aria-label="Commit summary"
                value={summary()}
                onInput={(e) => setSummary(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    void commit();
                  } else if (e.key === "Enter") {
                    e.preventDefault();
                    (e.currentTarget.nextElementSibling as HTMLElement | null)?.focus();
                  }
                }}
              />
              <textarea
                placeholder="Why this change…"
                aria-label="Commit description"
                rows={2}
                value={body()}
                onInput={(e) => setBody(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    void commit();
                  }
                }}
              />
            </div>
            <div class="gacts">
              <button
                class="gwrite"
                disabled={writing() || !staged().length}
                title={staged().length ? "Ask an agent (Claude Code, or Codex) to write the message from the staged changes" : "Stage changes first"}
                onClick={writeIt}
              >
                <Show when={writing()} fallback={<Icon name="spark" small />}>
                  <span class="spin" />
                </Show>
                {writing() ? "Writing…" : "Write it"}
              </button>
              <button class="gamend" aria-pressed={amend()} disabled={!status()!.head} title="Change the last commit instead of making a new one" onClick={toggleAmend}>
                Amend
              </button>
              <span class="spacer" />
              <span class="gcommit">
                <button
                  disabled={!summary().trim() || !canCommit() || !!busy() || needsUser()}
                  title={!canCommit() ? "Stage changes to commit" : undefined}
                  onClick={() => void commit()}
                >
                  <Show when={busy() === "commit"}>
                    <span class="spin" />
                  </Show>
                  {amend() ? "Amend" : "Commit"}
                  <kbd>⌘↩</kbd>
                </button>
                <button title="Commit options" aria-label="Commit options" onClick={(e) => setMenu({ kind: "commit", anchor: e.currentTarget })}>
                  <Icon name="chevron" small />
                </button>
              </span>
            </div>
          </div>
        </Show>
      </Show>
      <Show when={menu()}>
        {(open) => (
          <GitMenu anchor={open().anchor} onClose={() => setMenu(undefined)}>
            <Show when={open().kind === "branches"}>
              <div class="mm-h">Branches</div>
              <For each={status()?.branches ?? []}>
                {(branch) => (
                  <MenuItem
                    icon={branch === status()?.branch ? "check" : "branch"}
                    label={branch}
                    disabled={branch === status()?.branch}
                    run={() => act({ op: "switch", branch }, `Could not switch to ${branch}`)}
                  />
                )}
              </For>
              <Show when={remoteOnly(status()).length}>
                <div class="mm-sep" />
                <div class="mm-h">On the remote</div>
                <For each={remoteOnly(status())}>
                  {(branch) => <MenuItem icon="globe" label={branch} run={() => act({ op: "switch", branch }, `Could not switch to ${branch}`)} />}
                </For>
              </Show>
              <div class="mm-sep" />
              <MenuItem icon="plus" label="New branch…" run={newBranch} />
              <MenuItem
                icon="branch"
                label={`Merge into ${status()?.branch ?? "HEAD"}…`}
                disabled={!status()?.branch || !mergeable(status()).length || !!status()?.state}
                run={() => {
                  // After this menu closes on the click.
                  const anchor = open().anchor;
                  setTimeout(() => setMenu({ kind: "merge", anchor }));
                }}
              />
            </Show>
            <Show when={open().kind === "merge"}>
              <div class="mm-h">Merge into {status()?.branch}</div>
              <For each={mergeable(status())}>
                {(branch) => <MenuItem icon={branch.includes("/") && status()?.remote_branches.includes(branch) ? "globe" : "branch"} label={branch} run={() => merge(branch)} />}
              </For>
            </Show>
            <Show when={open().kind === "stash"}>
              <MenuItem icon="stash" label="Stash changes…" disabled={!status()?.files.length} run={stash} />
              <MenuItem
                icon="undo"
                label={status()?.stashes ? `Bring back the last stash (${status()!.stashes} kept)` : "Bring back the last stash"}
                disabled={!status()?.stashes}
                run={() => act({ op: "stash_pop" }, "Could not bring the stash back")}
              />
            </Show>
            <Show when={open().kind === "more"}>
              <MenuItem icon="plus" label="Stage all" disabled={!unstaged().length} run={() => stage(unstaged(), true)} />
              <MenuItem icon="minus" label="Unstage all" disabled={!staged().length} run={() => act({ op: "unstage", files: [] }, "Could not unstage")} />
              <MenuItem
                icon="discard"
                label="Throw away all unstaged changes…"
                disabled={!unstaged().length}
                run={() => discard(unstaged().filter((f) => !f.conflict))}
              />
              <div class="mm-sep" />
              <MenuItem icon="refresh" label="Read again" run={() => ((logRefs = undefined), readStatus())} />
            </Show>
            <Show when={open().kind === "commit"}>
              <MenuItem
                icon="push"
                label={amend() ? "Amend and push" : "Commit and push"}
                disabled={!summary().trim() || !canCommit()}
                run={() => commit("push")}
              />
              <MenuItem
                icon="plus"
                label="Stage all and commit"
                disabled={!summary().trim() || (!staged().length && !unstaged().length)}
                run={() => commit("all")}
              />
            </Show>
          </GitMenu>
        )}
      </Show>
    </div>
  );
}

function HeadButton(props: { icon: IconName; title: string; op: string; busy?: string; disabled?: boolean; count?: number; run: () => unknown }) {
  return (
    <button class="ib" title={props.title} aria-label={props.title} disabled={props.disabled || !!props.busy} onClick={() => void props.run()}>
      <Show when={props.busy === props.op} fallback={<Icon name={props.icon} small />}>
        <span class="spin" />
      </Show>
      <Show when={props.count}>
        <span class="n">{props.count}</span>
      </Show>
    </button>
  );
}

/** What GitHub makes of a file this big. */
function tooBig(bytes: number): string {
  return bytes > 100 * 1024 * 1024
    ? "GitHub refuses files over 100 MB, so pushing a commit with it would fail."
    : "GitHub warns about files over 50 MB.";
}

/** Branches that can be merged into the one checked out: the other local ones, then the remote ones. */
function mergeable(status?: GitStatus): string[] {
  if (!status) return [];
  return [...status.branches.filter((b) => b !== status.branch), ...status.remote_branches];
}

/** Remote branches with no local branch of the same name. */
function remoteOnly(status?: GitStatus): string[] {
  if (!status) return [];
  return status.remote_branches.filter((r) => !status.branches.includes(r.slice(r.indexOf("/") + 1)));
}

function RefBadge(props: { badge: Badge }) {
  const b = props.badge;
  const icon: IconName = b.kind === "head" ? "check" : b.kind === "tag" ? "tag" : b.kind === "remote" ? "globe" : "branch";
  const title = b.kind === "head" ? `${b.name}, checked out` : b.kind === "tag" ? `Tag ${b.name}` : b.kind === "remote" ? `${b.name}, on the remote` : `Branch ${b.name}`;
  return (
    <span class={`gref ${b.kind === "head" ? "head" : b.kind === "branch" ? "br" : b.kind === "tag" ? "tag" : "rem"}`} title={b.remote ? `${title}; the remote has it too` : title}>
      <Icon name={icon} small />
      <span class="gn">{b.name}</span>
      <Show when={b.remote}>
        <i>
          <Icon name="globe" small />
        </i>
      </Show>
    </span>
  );
}

function FileGroup(props: {
  title: string;
  dot: string;
  files: GitFile[];
  staged?: boolean;
  busy: boolean;
  onAll: () => void;
  onToggle: (file: GitFile) => void;
  onDiscard?: (file: GitFile) => void;
  /** Settles a conflicted file with one side's version of it. */
  onResolve?: (file: GitFile, side: "ours" | "theirs") => void;
  onDiff: (file: GitFile) => void;
  onOpen: (file: GitFile) => void;
}) {
  return (
    <>
      <div class="gsec">
        <span class={`dot ${props.dot}`} />
        <span class="gsec-t">{props.title}</span>
        <span class="gcount">{props.files.length}</span>
        <span class="spacer" />
        <button class="gfa" disabled={props.busy} title={props.staged ? "Unstage all" : "Stage all"} onClick={props.onAll}>
          <Icon name={props.staged ? "minus" : "plus"} small />
        </button>
      </div>
      <For each={props.files}>
        {(file) => {
          const mark = fileMark(file, !!props.staged);
          const where = splitPath(file.path);
          const lines = props.staged ? file.staged_lines : file.lines;
          const big = (file.size ?? 0) >= BIG_FILE && (!props.staged || file.staged === "A");
          return (
            <div class="gfile" classList={{ conflict: !!props.onResolve && file.conflict }} title={`${file.path}\n${mark.title}`} onClick={() => props.onDiff(file)}>
              <span class={`gst ${mark.kind}`}>{mark.letter}</span>
              <FileIcon name={where.name.replace(/\/$/, "")} dir={file.path.endsWith("/")} />
              <span class="gname">{where.name}</span>
              <span class="gdir">
                <span>{where.dir}</span>
              </span>
              <Show when={big}>
                <span class="gbig" title={tooBig(file.size!)}>
                  {fileSize(file.size!)}
                </span>
              </Show>
              <span class="gadd">{lineCounts(lines)}</span>
              <span class="gfacts">
                <Show when={!file.path.endsWith("/") && mark.letter !== "D"}>
                  <button
                    title="Open the file"
                    aria-label={`Open ${file.path}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onOpen(file);
                    }}
                  >
                    <Icon name="open" small />
                  </button>
                </Show>
                <Show when={props.onResolve && file.conflict}>
                  <button
                    class="gside"
                    title="Keep this branch's version of the whole file"
                    disabled={props.busy}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onResolve!(file, "ours");
                    }}
                  >
                    Mine
                  </button>
                  <button
                    class="gside"
                    title="Take the merged branch's version of the whole file"
                    disabled={props.busy}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onResolve!(file, "theirs");
                    }}
                  >
                    Theirs
                  </button>
                </Show>
                <Show when={props.onDiscard && !file.conflict}>
                  <button
                    title={file.unstaged === "?" ? "Delete" : "Throw away the changes"}
                    aria-label={`Throw away ${file.path}`}
                    disabled={props.busy}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onDiscard!(file);
                    }}
                  >
                    <Icon name="discard" small />
                  </button>
                </Show>
                <button
                  title={props.staged ? "Unstage" : "Stage"}
                  aria-label={`${props.staged ? "Unstage" : "Stage"} ${file.path}`}
                  disabled={props.busy}
                  onClick={(e) => {
                    e.stopPropagation();
                    props.onToggle(file);
                  }}
                >
                  <Icon name={props.staged ? "minus" : "plus"} small />
                </button>
              </span>
            </div>
          );
        }}
      </For>
    </>
  );
}

function CommitDetail(props: { info?: GitCommitInfo; onClose: () => void; onDiff: (file: string) => void }) {
  return (
    <div class="gstage gdetail">
      <Show when={props.info} fallback={<div class="var-note">Reading the commit…</div>}>
        {(info) => {
          const face = avatar({ author: info().author, email: info().email, coauthors: coauthorsOf(info().message) });
          return (
            <>
              <div class="gd-head">
                <span class="gsha" title={info().sha}>
                  {info().sha.slice(0, 7)}
                </span>
                <span class="gd-who">{face.who}</span>
                <span class="gd-when">{new Date(info().time_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</span>
                <span class="spacer" />
                <button class="gfa" title="Back to the working tree" aria-label="Close" onClick={props.onClose}>
                  <Icon name="x" small />
                </button>
              </div>
              <div class="gd-msg">{info().message}</div>
              <div class="gsec">
                <span class="gsec-t">Changed</span>
                <span class="gcount">{info().files.length}</span>
              </div>
              <div class="gfiles">
                <For each={info().files}>
                  {(change) => {
                    const where = splitPath(change.path);
                    const mark = fileMark({ path: change.path, staged: change.status, from: change.from }, true);
                    return (
                      <div class="gfile" title={`${change.path}\n${mark.title}`} onClick={() => props.onDiff(change.path)}>
                        <span class={`gst ${mark.kind}`}>{mark.letter}</span>
                        <FileIcon name={where.name} />
                        <span class="gname">{where.name}</span>
                        <span class="gdir">
                <span>{where.dir}</span>
              </span>
                        <span class="gadd">{lineCounts(change.lines)}</span>
                      </div>
                    );
                  }}
                </For>
              </div>
            </>
          );
        }}
      </Show>
    </div>
  );
}

/** The co-author trailers of a whole message. */
function coauthorsOf(message: string): string[] {
  return [...message.matchAll(/^co-authored-by:\s*(.+)$/gim)].map((m) => m[1].trim());
}

function GitMenu(props: { anchor: HTMLElement; onClose: () => void; children: JSX.Element }) {
  let menu!: HTMLDivElement;
  closeOnOutside(() => menu, props.anchor, props.onClose);
  const box = props.anchor.getBoundingClientRect();
  const [place, setPlace] = createSignal({ left: box.left, top: box.bottom + 4 });
  // Under the button, from its left edge, kept inside the window.
  queueMicrotask(() => {
    const left = Math.max(8, Math.min(box.left, window.innerWidth - menu.offsetWidth - 8));
    const below = box.bottom + 4 + menu.offsetHeight < window.innerHeight - 8;
    setPlace({ left, top: below ? box.bottom + 4 : Math.max(8, box.top - 4 - menu.offsetHeight) });
  });
  return (
    <div ref={menu} class="kpop gmenu" role="menu" style={{ left: `${place().left}px`, top: `${place().top}px` }} onClick={(e) => e.target !== e.currentTarget && e.target instanceof HTMLElement && e.target.closest("button") && props.onClose()}>
      {props.children}
    </div>
  );
}

function MenuItem(props: { icon: IconName; label: string; disabled?: boolean; run: () => unknown }) {
  return (
    <button class="mm-i" role="menuitem" disabled={props.disabled} onClick={() => void props.run()}>
      <Icon name={props.icon} small />
      <span>{props.label}</span>
      <kbd />
    </button>
  );
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
