import { createEffect, createSignal, For, on, onCleanup, Show, untrack } from "solid-js";
import type { SessionInfo, SystemStats, Variable, Variables } from "../backend/types";
import { formatBytes, languageLabel } from "../model/machine";
import { groupVariables, pathKey, removedText, sizeOf } from "../model/variables";
import type { Workspace } from "../model/workspace";
import { notify } from "./dialogs";
import { Icon } from "./icons";
import { SessionPicker } from "./KernelBar";

/** How long after a run finishes the list is read again, so a burst of runs reads it once. */
const REFRESH_MS = 250;
/** What the daemon says when the session runs other code. */
const BUSY = "the session is busy running code";

/**
 * The Variables pane: the variables of the session in front (the Console's),
 * grouped as data, values and functions, with their parts a click away (a
 * data frame's columns, an AnnData's obs and var). Ticked variables can be
 * deleted to free their memory. The list is read again after each run in the
 * session; while the session is busy, the last list stays.
 */
export function VariablesPane(props: {
  ws: Workspace;
  session?: SessionInfo;
  sessions: SessionInfo[];
  usedBy?: (session: string) => string[];
  onSession: (session: string) => void;
  /** Opens a data frame or matrix, by its path, in the Data Explorer. */
  onView?: (path: string[]) => void;
  /** False while the pane is hidden: nothing is read then. */
  shown: boolean;
  /** Reads the list again when this changes (the refresh button). */
  refreshKey?: number;
}) {
  const ws = props.ws;
  const [listed, setListed] = createSignal<Variables>();
  const [error, setError] = createSignal<string>();
  const [filter, setFilter] = createSignal("");
  /** Open variables and parts, by path. */
  const [open, setOpen] = createSignal<ReadonlySet<string>>(new Set());
  /** Each open path's parts, or why they couldn't be read. */
  const [parts, setParts] = createSignal<Record<string, Variables | string>>({});
  const [picked, setPicked] = createSignal<ReadonlySet<string>>(new Set());
  const [removed, setRemoved] = createSignal<string>();
  const [memory, setMemory] = createSignal<number>();
  const id = () => props.session?.id;
  const busy = () => {
    const session = id();
    return !!session && ws.isBusy(session);
  };
  const name = () => (props.session ? languageLabel(props.session.language) : "");

  let generation = 0;
  const list = (session: string, path: string[] = []) => ws.call<Variables>("list_variables", { session, path });

  async function refresh() {
    const session = id();
    if (!session || !props.shown || ws.isBusy(session)) return;
    const mine = ++generation;
    try {
      const all = await list(session);
      if (mine !== generation) return;
      setListed(all);
      setError(undefined);
      // Open parts are read again too, as long as their variable is still there.
      const names = new Set(all.variables.map((v) => v.name));
      const next: Record<string, Variables | string> = {};
      for (const key of open()) {
        const path = key.split("\u0000");
        if (!names.has(path[0])) continue;
        next[key] = await list(session, path).catch((err) => String(err));
      }
      if (mine === generation) setParts(next);
    } catch (err) {
      // Code started meanwhile: the list is read again when it finishes.
      if (mine === generation && !String(err).includes(BUSY)) setError(String(err));
    }
    const stats = await ws.call<SystemStats>("system_stats", {}).catch(() => undefined);
    if (mine === generation) setMemory(stats?.sessions.find((m) => m.session === session)?.rss);
  }

  // A new session starts from scratch.
  createEffect(
    on(id, () => {
      generation++;
      setListed(undefined);
      setError(undefined);
      setOpen(new Set<string>());
      setParts({});
      setPicked(new Set<string>());
      setRemoved(undefined);
      setMemory(undefined);
    }),
  );
  createEffect(() => {
    id();
    props.refreshKey;
    // Untracked: what refresh() reads (whether the session is busy) mustn't start it again.
    if (props.shown) untrack(() => void refresh());
  });
  // After each run in the session, once it is idle.
  let timer: ReturnType<typeof setTimeout> | undefined;
  const unwatch = ws.runs.watchAll((run) => {
    if (run.session !== id() || run.quiet || run.phase !== "done") return;
    setRemoved(undefined);
    clearTimeout(timer);
    timer = setTimeout(() => void refresh(), REFRESH_MS);
  });
  onCleanup(() => {
    unwatch();
    clearTimeout(timer);
  });

  const toggleOpen = async (path: string[]) => {
    const key = pathKey(path);
    const now = new Set(open());
    if (now.has(key)) {
      now.delete(key);
      setOpen(now);
      return;
    }
    now.add(key);
    setOpen(now);
    const session = id();
    if (!session || parts()[key]) return;
    const read = await list(session, path).catch((err) => String(err));
    if (session === id()) setParts((all) => ({ ...all, [key]: read }));
  };

  const togglePicked = (name: string) => {
    const now = new Set(picked());
    if (now.has(name)) now.delete(name);
    else now.add(name);
    setPicked(now);
    setRemoved(undefined);
  };

  const pickedBytes = () => sizeOf(listed()?.variables ?? [], picked());

  async function remove() {
    const session = id();
    const names = [...picked()];
    if (!session || !names.length) return;
    const bytes = pickedBytes();
    try {
      await ws.call("delete_variables", { session, names });
      setPicked(new Set<string>());
      setRemoved(removedText(names, bytes));
      await refresh();
    } catch (err) {
      notify(`Could not delete ${names.join(", ")}: ${String(err)}`);
    }
  }

  const Row = (row: { variable: Variable; path: string[] }) => {
    const v = row.variable;
    const key = pathKey(row.path);
    const top = row.path.length === 1;
    const isOpen = () => open().has(key);
    return (
      <>
        <div
          class="var"
          classList={{ picked: top && picked().has(v.name), child: !top, opens: v.children }}
          style={{ "padding-left": `${6 + (row.path.length - 1) * 14}px` }}
          onClick={() => v.children && void toggleOpen(row.path)}
        >
          <Show when={top} fallback={<span />}>
            <input
              type="checkbox"
              class="vck"
              checked={picked().has(v.name)}
              aria-label={`Select ${v.name}`}
              onClick={(e) => e.stopPropagation()}
              onChange={() => togglePicked(v.name)}
            />
          </Show>
          <span class="tw">
            <Show when={v.children}>
              <Icon name={isOpen() ? "chevron" : "chevronRight"} small />
            </Show>
          </span>
          <span class="nm" title={v.name}>
            {v.name}
          </span>
          <span class="sm" title={`${v.summary} (${v.class})`}>
            {v.summary} <em>{v.class}</em>
          </span>
          <span class="sz">{v.size !== undefined ? formatBytes(v.size) : ""}</span>
          <Show when={v.table && props.onView} fallback={<span />}>
            <button
              class="vw"
              title="Open in the Data Explorer"
              aria-label={`Open ${row.path.join(" ")} in the Data Explorer`}
              onClick={(e) => {
                e.stopPropagation();
                props.onView?.(row.path);
              }}
            >
              <Icon name="table" small />
            </button>
          </Show>
        </div>
        <Show when={isOpen()}>
          <Show
            when={parts()[key]}
            fallback={
              <div class="var-note" style={{ "padding-left": `${36 + (row.path.length - 1) * 14}px` }}>
                Reading…
              </div>
            }
          >
            {(read) => {
              const value = read();
              if (typeof value === "string") {
                return (
                  <div class="var-note err" style={{ "padding-left": `${36 + (row.path.length - 1) * 14}px` }}>
                    {value}
                  </div>
                );
              }
              return (
                <>
                  <For each={value.variables}>{(child) => <Row variable={child} path={[...row.path, child.name]} />}</For>
                  <Show when={value.more}>
                    <div class="var-note" style={{ "padding-left": `${36 + row.path.length * 14}px` }}>
                      … {value.more.toLocaleString()} more
                    </div>
                  </Show>
                </>
              );
            }}
          </Show>
        </Show>
      </>
    );
  };

  return (
    <div class="vars">
      <Show
        when={picked().size}
        fallback={
          <div class="vtool">
            <SessionPicker ws={ws} session={props.session} sessions={props.sessions} usedBy={props.usedBy} onPick={props.onSession} />
            <input
              class="vfilter"
              placeholder="Filter"
              aria-label="Filter variables"
              value={filter()}
              onInput={(e) => setFilter(e.currentTarget.value)}
            />
            <Show when={memory()}>
              {(rss) => (
                <span class="vmem" title={`Memory used by this ${name()} session`}>
                  {formatBytes(rss())}
                </span>
              )}
            </Show>
          </div>
        }
      >
        <div class="vtool vsel">
          <span class="vcount">
            {picked().size} selected{pickedBytes() ? ` · ${formatBytes(pickedBytes())}` : ""}
          </span>
          <span class="spacer" />
          <button class="vbtn" onClick={() => setPicked(new Set<string>())}>
            Cancel
          </button>
          <button class="vbtn danger" disabled={busy()} title={busy() ? `${name()} is busy` : `Delete with ${props.session?.language.toLowerCase() === "r" ? "rm() and gc()" : "del and gc.collect()"}`} onClick={() => void remove()}>
            <Icon name="trash" small />
            Delete
          </button>
        </div>
      </Show>
      <Show when={removed()}>
        <div class="vfreed">
          <Icon name="check" small />
          {removed()}
        </div>
      </Show>
      <div class="vlist">
        <Show when={props.session} fallback={<p class="vempty">Start R or Python in the Console, or run some code, and its variables show here.</p>}>
          <Show when={busy()}>
            <div class="vbusy">{name()} is running code; the list updates when it finishes.</div>
          </Show>
          <Show when={error()}>
            <div class="var-note err">{error()}</div>
          </Show>
          <Show when={listed()}>
            {(all) => (
              <Show when={all().variables.length} fallback={<p class="vempty">No variables yet.</p>}>
                <For each={groupVariables(all().variables, filter())}>
                  {(group) => (
                    <>
                      <div class="vh">{group.title}</div>
                      <For each={group.variables}>{(v) => <Row variable={v} path={[v.name]} />}</For>
                    </>
                  )}
                </For>
              </Show>
            )}
          </Show>
        </Show>
      </div>
    </div>
  );
}
