import { createEffect, createMemo, createResource, createSignal, For, type JSX, on, onCleanup, Show } from "solid-js";
import type { AgentInfo, AgentOptions, TerminalInfo } from "../backend/types";
import type { Doc } from "../docs/doc";
import {
  AGENT_KINDS,
  type AgentKind,
  agentKind,
  currentPermission,
  documentPath,
  effortLevel,
  effortName,
  type ModelChoice,
  modelLabel,
  PERMISSIONS,
  permission,
  SUBMIT_DELAY_MS,
  tokens,
} from "../model/agents";
import type { TerminalStore } from "../model/terminals";
import type { Workspace } from "../model/workspace";
import { ask, notify } from "./dialogs";
import { EffortControl } from "./EffortControl";
import { FileIcon, Icon } from "./icons";
import { type Pane, PaneButtons } from "./PaneButtons";
import { TermView } from "./termView";

const COMPOSER_MIN = 34;
const COMPOSER_MAX = 420;
/** Effort is sent to the agent once the knob has rested this long. */
const EFFORT_SETTLE_MS = 600;

/**
 * The Agents pane: a tab per agent session in this project. Claude Code and
 * Codex are their real terminals, kept running by the daemon, with a
 * message box under them.
 */
export function AgentsPane(props: {
  ws: Workspace;
  store: TerminalStore;
  project: string;
  hostLabel: string;
  docs: Doc[];
  active?: Doc;
  selected?: string;
  onSelect: (id: string | undefined) => void;
  width: number;
  min: boolean;
  max: boolean;
  onMin: (p: Pane) => void;
  onMax: (p: Pane) => void;
  /** Runs a command in the Console's Terminal tab, to install an agent. */
  onInstall: (command: string) => void;
  /** Opens a file in the editor and brings it to the front. */
  onOpen: (path: string) => void;
}) {
  const store = props.store;
  const [picking, setPicking] = createSignal(false);
  /** The view of a session being started, before the daemon has it. */
  const [starting, setStarting] = createSignal<TermView>();
  const views = new Map<string, TermView>();
  let body!: HTMLDivElement;
  let tabScroll!: HTMLDivElement;

  const current = (): TerminalInfo | undefined => {
    const agents = store.agents();
    return agents.find((t) => t.id === props.selected) ?? agents[agents.length - 1];
  };

  const viewFor = (id: string): TermView => {
    let view = views.get(id);
    if (!view) {
      view = new TermView(store, (url) => props.ws.backend.openLink(url));
      views.set(id, view);
    }
    return view;
  };

  // Show the current session's terminal; keep the others' alive but hidden.
  // While a session starts, its own view shows, not one made for it here.
  const shown = createMemo(() => (picking() || starting() ? undefined : current()?.id));
  createEffect(
    on(shown, (id) => {
      for (const [other, view] of views) view.el.hidden = other !== id;
      if (!id) return;
      const view = viewFor(id);
      view.el.hidden = false;
      void view.mount(body).then(() => {
        view.bind(id);
        if (!props.min) view.focus();
      });
    }),
  );
  // Sessions that were closed, here or elsewhere, take their views with them.
  createEffect(() => {
    const ids = new Set(store.list().map((t) => t.id));
    for (const [id, view] of views) {
      if (ids.has(id) || view === starting()) continue;
      view.dispose();
      views.delete(id);
    }
  });
  onCleanup(() => {
    for (const view of views.values()) view.dispose();
  });
  // The New session tab is the last one: bring it into view.
  createEffect(
    on(picking, (on) => {
      if (on) queueMicrotask(() => tabScroll.scrollTo?.({ left: tabScroll.scrollWidth }));
    }),
  );

  async function start(kind: AgentKind, options: AgentOptions) {
    // A view made first, so the agent starts at the size it will be shown at.
    const view = new TermView(store, (url) => props.ws.backend.openLink(url));
    setStarting(view);
    setPicking(false);
    for (const other of views.values()) other.el.hidden = true;
    await view.mount(body);
    try {
      const info = await store.start(kind.kind, options, view.size());
      views.set(info.id, view);
      view.bind(info.id);
      props.onSelect(info.id);
      view.focus();
    } catch (err) {
      view.dispose();
      notify(`Could not start ${kind.name}: ${message(err)}`);
      setPicking(true);
    } finally {
      setStarting(undefined);
    }
  }

  async function end(terminal: TerminalInfo) {
    if (terminal.running) {
      const choice = await ask(
        `End this ${terminal.title} session?`,
        ["End session", "Cancel"],
        "The agent stops, along with anything it is running.",
      );
      if (choice !== "End session") return;
    }
    try {
      await store.close(terminal.id);
    } catch (err) {
      notify(`Could not end the session: ${message(err)}`);
    }
  }

  return (
    <div
      class="pane agents-pane"
      classList={{ min: props.min, max: props.max }}
      style={{ width: `${props.width}px` }}
      aria-label="Agents"
    >
      <div class="tabs" role="tablist" data-tauri-drag-region="deep">
        <div ref={tabScroll} class="tab-scroll">
          <For each={store.agents()}>
            {(terminal) => (
              <div
                class="tab atab"
                role="tab"
                aria-selected={!picking() && current()?.id === terminal.id}
                classList={{ active: !picking() && current()?.id === terminal.id, ended: !terminal.running }}
                title={tabTitle(terminal)}
                aria-label={modelLabel(terminal)}
                onClick={() => {
                  setPicking(false);
                  props.onSelect(terminal.id);
                }}
              >
                <span class="prov-wrap">
                  <ProviderIcon kind={terminal.kind} />
                  <Show when={terminal.running && (terminal.agent?.state === "waiting" || terminal.agent?.state === "busy")}>
                    <span class="dot" classList={{ wait: terminal.agent?.state === "waiting", busy: terminal.agent?.state === "busy" }} />
                  </Show>
                </span>
                <button
                  class="tab-close"
                  title="End session"
                  onClick={(e) => {
                    e.stopPropagation();
                    void end(terminal);
                  }}
                >
                  <Icon name="x" small />
                </button>
              </div>
            )}
          </For>
          <Show when={picking()}>
            <div class="tab active" role="tab" aria-selected="true">
              New session
            </div>
          </Show>
        </div>
        <button class="pane-button" title="New session" onClick={() => setPicking(true)}>
          <Icon name="plus" small />
        </button>
        <PaneButtons pane="agents" min={props.min} max={props.max} onMin={props.onMin} onMax={props.onMax} />
      </div>
      <Show when={!picking() && current()}>
        {(terminal) => <InfoLine terminal={terminal()} hostLabel={props.hostLabel} project={props.project} onOpen={props.onOpen} />}
      </Show>
      <div class="agent-body">
        <Show when={picking()}>
          <Picker
            store={store}
            docs={props.docs}
            active={props.active}
            project={props.project}
            hostLabel={props.hostLabel}
            onStart={(kind, options) => void start(kind, options)}
            onCancel={() => setPicking(false)}
            onInstall={props.onInstall}
            canCancel={store.agents().length > 0}
          />
        </Show>
        <Show when={!picking() && !current() && !starting()}>
          <div class="agent-empty">
            <div class="ae-ico">
              <Icon name="chat" />
            </div>
            <b>No agent sessions</b>
            <p>
              Each session runs Claude Code or Codex in a terminal on {props.hostLabel}, in this project's folder. They can
              read your R and Python sessions through Sandhill's tools, and keep running when you close the window.
            </p>
            <button class="primary" onClick={() => setPicking(true)}>
              <Icon name="plus" small />
              New session
            </button>
          </div>
        </Show>
        <div class="term-host" ref={body} hidden={picking() || (!current() && !starting())} />
        <Show when={!picking() && current() && !current()!.running}>
          <Ended terminal={current()!} onClose={() => void end(current()!)} onAgain={(t) => void start(agentKind(t.kind)!, t.options ?? {})} />
        </Show>
      </div>
      <Show when={!picking() && current()?.running && current()}>
        {(terminal) => (
          <Composer
            terminal={terminal()}
            store={store}
            view={() => views.get(terminal().id)}
            active={props.active}
            project={props.project}
          />
        )}
      </Show>
    </div>
  );
}

function tabTitle(terminal: TerminalInfo): string {
  const parts = [`${terminal.title} · ${modelLabel(terminal)}`];
  const docs = terminal.options?.documents;
  if (docs?.length) parts.push(docs.join(", "));
  if (!terminal.running) parts.push("ended");
  else if (terminal.agent?.state === "waiting") parts.push(terminal.agent.message ?? "waiting for you");
  else if (terminal.agent?.state === "busy") parts.push("working");
  return parts.join(" · ");
}

/** Claude's mark, OpenAI's, or a chip for anything else. */
export function ProviderIcon(props: { kind: string }) {
  return (
    <Show
      when={props.kind === "claude"}
      fallback={
        <svg class="ico prov" viewBox="0 0 16 16" aria-hidden="true">
          <Show when={props.kind === "codex"} fallback={<rect x="3" y="3" width="10" height="10" rx="2" />}>
            <g stroke-width="1.15">
              <path d="M8 2.2 13 5.1v5.8L8 13.8 3 10.9V5.1z" />
              <path d="M8 2.2c-2.1 1.3-2.1 4.4 0 5.8M13 5.1c-2.2-1.2-4.9.4-5 2.9M13 10.9c0-2.5-2.7-4.1-5-2.9M8 13.8c2.1-1.3 2.1-4.4 0-5.8M3 10.9c2.2 1.2 4.9-.4 5-2.9M3 5.1c0 2.5 2.7 4.1 5 2.9" />
            </g>
          </Show>
        </svg>
      }
    >
      <svg class="ico prov claude" viewBox="0 0 16 16" aria-hidden="true">
        <path
          stroke-width="1.7"
          d="M8 1.6v4.2M8 10.2v4.2M1.6 8h4.2M10.2 8h4.2M3.5 3.5l3 3M9.5 9.5l3 3M3.5 12.5l3-3M9.5 6.5l3-3M5.2 2.3l1.4 3.9M9.4 9.8l1.4 3.9M2.3 10.8l3.9-1.4M9.8 6.6l3.9-1.4"
        />
      </svg>
    </Show>
  );
}

/** ">_ claude in a terminal on gpu01 · Opus 5.5", then the documents it is about. */
function InfoLine(props: { terminal: TerminalInfo; hostLabel: string; project: string; onOpen: (path: string) => void }) {
  const kind = () => agentKind(props.terminal.kind);
  return (
    <div class="agent-info">
      <span class="ai-l">
        <Icon name="command" small />
        {kind()?.program ?? props.terminal.kind} in a terminal on {props.hostLabel} · {modelLabel(props.terminal)}
      </span>
      <For each={props.terminal.options?.documents ?? []}>
        {(doc) => (
          <button class="dchip" title={`Open ${doc}`} onClick={() => props.onOpen(documentPath(props.project, doc))}>
            <FileIcon name={doc} />
            {doc}
          </button>
        )}
      </For>
    </div>
  );
}

function Ended(props: { terminal: TerminalInfo; onClose: () => void; onAgain: (t: TerminalInfo) => void }) {
  const how = () => {
    const exit = props.terminal.exit;
    if (exit?.signal) return `was stopped (signal ${exit.signal})`;
    if (exit?.code) return `ended with exit code ${exit.code}`;
    return "ended";
  };
  return (
    <div class="agent-ended">
      <span>
        {props.terminal.title} {how()}.
      </span>
      <span class="spacer" />
      <button onClick={() => props.onAgain(props.terminal)}>Start again</button>
      <button onClick={props.onClose}>Close</button>
    </div>
  );
}

/** New session: which agent and model, what it is about, and what it may do unasked. */
function Picker(props: {
  store: TerminalStore;
  docs: Doc[];
  active?: Doc;
  project: string;
  hostLabel: string;
  canCancel: boolean;
  onStart: (kind: AgentKind, options: AgentOptions) => void;
  onCancel: () => void;
  onInstall: (command: string) => void;
}) {
  const [installed] = createResource(() => props.store.listAgents().catch(() => [] as AgentInfo[]));
  const info = (kind: string) => installed()?.find((a) => a.kind === kind);
  const [choice, setChoice] = createSignal({ kind: "claude", model: "opus" as string | undefined });
  const [effort, setEffort] = createSignal<string>();
  const [perm, setPerm] = createSignal("supervised");
  const relative = (path: string) => (path.startsWith(`${props.project}/`) ? path.slice(props.project.length + 1) : path);
  const [docs, setDocs] = createSignal<string[]>(props.active ? [relative(props.active.path)] : []);
  const kind = () => agentKind(choice().kind)!;
  const missing = () => installed() && !info(choice().kind)?.path;
  const toggleDoc = (doc: string, on: boolean) => setDocs((list) => (on ? [...list, doc] : list.filter((d) => d !== doc)));
  const go = () =>
    props.onStart(kind(), {
      model: choice().model,
      effort: effort(),
      permission: perm(),
      documents: docs().length ? docs() : undefined,
    });
  return (
    <div class="picker">
      <div class="pk-h">Agent and model</div>
      <For each={AGENT_KINDS}>
        {(agent) => (
          <div class="pk-group">
            <div class="pk-g">
              <ProviderIcon kind={agent.kind} />
              {agent.name}
              <small>
                {installed.loading
                  ? agent.plan
                  : info(agent.kind)?.path
                    ? `${agent.plan}${info(agent.kind)?.version ? ` · ${info(agent.kind)!.version}` : ""}`
                    : `Not installed on ${props.hostLabel}`}
              </small>
            </div>
            <For each={agent.models}>
              {(model) => (
                <label class="pk-opt" classList={{ on: choice().kind === agent.kind && choice().model === model.id }}>
                  <input
                    type="radio"
                    name="pk-model"
                    checked={choice().kind === agent.kind && choice().model === model.id}
                    onChange={() => {
                      if (choice().kind !== agent.kind) setEffort(undefined);
                      setChoice({ kind: agent.kind, model: model.id });
                    }}
                  />
                  {model.label}
                </label>
              )}
            </For>
          </div>
        )}
      </For>
      <Show when={missing()}>
        <div class="pk-install">
          <span>
            {kind().name} isn't installed on {props.hostLabel}. Install it with <code>{info(choice().kind)?.install}</code>
          </span>
          <button class="primary" onClick={() => props.onInstall(info(choice().kind)!.install)}>
            Install in a terminal
          </button>
        </div>
      </Show>
      <EffortControl levels={kind().efforts} value={effort()} recommended={kind().recommended} onChange={setEffort} />
      <div class="pk-h">Refers to</div>
      <For each={props.docs.map((d) => relative(d.path))}>
        {(doc) => (
          <label class="pk-opt" classList={{ on: docs().includes(doc) }}>
            <input type="checkbox" checked={docs().includes(doc)} onChange={(e) => toggleDoc(doc, e.currentTarget.checked)} />
            <FileIcon name={doc} />
            {doc}
          </label>
        )}
      </For>
      <label class="pk-opt" classList={{ on: !docs().length }}>
        <input type="checkbox" checked={!docs().length} onChange={(e) => e.currentTarget.checked && setDocs([])} />
        <FileIcon name="project" dir />
        Whole project
      </label>
      <div class="pk-h">Permissions</div>
      <For each={PERMISSIONS}>
        {(p) => (
          <label class="pk-opt pk-perm" classList={{ on: perm() === p.id }}>
            <input type="radio" name="pk-perm" checked={perm() === p.id} onChange={() => setPerm(p.id)} />
            <span>
              <b>{p.label}</b>
              <small>{p.detail}</small>
            </span>
          </label>
        )}
      </For>
      <p class="pk-note">
        The session starts in this project's folder on {props.hostLabel}, signed in with your own {kind().name} account. It can
        reach your R and Python sessions through Sandhill's tools.
      </p>
      <div class="pk-act">
        <Show when={props.canCancel}>
          <button onClick={props.onCancel}>Cancel</button>
        </Show>
        <button class="primary" disabled={!!missing()} onClick={go}>
          Start session
        </button>
      </div>
    </div>
  );
}

/** The message box under a terminal, and the model and permission controls. */
function Composer(props: {
  terminal: TerminalInfo;
  store: TerminalStore;
  view: () => TermView | undefined;
  active?: Doc;
  project: string;
}) {
  const kind = () => agentKind(props.terminal.kind);
  const [height, setHeight] = createSignal(COMPOSER_MIN);
  const [popover, setPopover] = createSignal<"model" | "perm" | "context">();
  /** The effort just picked here, until the agent reports one. */
  const [effortChoice, setEffortChoice] = createSignal<string>();
  const effort = () => effortChoice() ?? effortLevel(props.terminal);
  createEffect(on(() => props.terminal.agent?.status?.effort, () => setEffortChoice(undefined), { defer: true }));
  let effortTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(effortTimer));
  // A slash command typed while the agent asks a question would answer it, so
  // commands wait until the question is answered.
  const waiting = () => props.terminal.agent?.state === "waiting";
  let held: (() => void) | undefined;
  const whenFree = (run: () => void) => {
    if (waiting()) held = run;
    else run();
  };
  createEffect(
    on(waiting, (now) => {
      if (now || !held) return;
      const run = held;
      held = undefined;
      run();
    }),
  );
  let input!: HTMLTextAreaElement;

  const send = () => {
    const text = input.value.replace(/\s+$/, "");
    const view = props.view();
    if (!text || !view) return;
    view.paste(text);
    // Enter on its own, once the paste has landed.
    setTimeout(() => void props.store.input(props.terminal.id, "\r"), SUBMIT_DELAY_MS);
    input.value = "";
  };
  /** Sets Claude Code's effort with /effort once the knob settles, leaving the popup open. */
  const changeEffort = (level: string) => {
    setEffortChoice(level);
    clearTimeout(effortTimer);
    effortTimer = setTimeout(() => whenFree(() => type(`/effort ${level}`)), EFFORT_SETTLE_MS);
  };
  /** Types `text` into the agent, then Enter. */
  const type = (text: string) => {
    void props.store.input(props.terminal.id, text);
    setTimeout(() => void props.store.input(props.terminal.id, "\r"), SUBMIT_DELAY_MS);
  };
  /** Runs a slash command in the agent, once it isn't waiting for an answer. */
  const command = (text: string) => {
    whenFree(() => type(text));
    setPopover(undefined);
    props.view()?.focus();
  };
  const addContext = () => {
    const doc = props.active;
    if (!doc) return;
    const path = doc.path.startsWith(`${props.project}/`) ? doc.path.slice(props.project.length + 1) : doc.path;
    const at = input.selectionStart ?? input.value.length;
    const before = input.value.slice(0, at);
    const mention = `${before && !/\s$/.test(before) ? " " : ""}@${path} `;
    input.setRangeText(mention, at, input.selectionEnd ?? at, "end");
    input.focus();
  };
  const grip = (down: PointerEvent) => {
    down.preventDefault();
    const target = down.currentTarget as HTMLElement;
    target.setPointerCapture(down.pointerId);
    const start = height();
    const move = (e: PointerEvent) => setHeight(Math.max(COMPOSER_MIN, Math.min(COMPOSER_MAX, start + down.clientY - e.clientY)));
    const up = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", up);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
  };
  const closePopover = (e: MouseEvent) => {
    if (!(e.target as Element).closest(".apop, .cs, .ctxline")) setPopover(undefined);
  };
  document.addEventListener("mousedown", closePopover);
  onCleanup(() => document.removeEventListener("mousedown", closePopover));

  const status = () => props.terminal.agent?.status;
  const perm = () => currentPermission(props.terminal);
  const started = () => permission(props.terminal.options?.permission);

  return (
    <div class="composer">
      <Show when={status()?.context !== undefined}>
        <button
          class="ctxline"
          title={`Context: ${status()!.context}% used. Click to compact.`}
          onClick={() => setPopover(popover() === "context" ? undefined : "context")}
        >
          <span class="cl-pct">{Math.round(status()!.context!)}%</span>
          <span class="cl-track">
            <i classList={{ warn: status()!.context! >= 70 }} style={{ width: `${Math.max(1, status()!.context!)}%` }} />
          </span>
          <Show when={status()!.context_size}>
            <span class="cl-max">{tokens(status()!.context_size!)}</span>
          </Show>
        </button>
      </Show>
      <div class="cbox">
        <span class="grip" title="Drag to resize the message box" onPointerDown={grip} />
        <textarea
          ref={input}
          class="ain"
          rows={1}
          style={{ height: `${height()}px` }}
          placeholder={kind()?.placeholder ?? "Message the agent"}
          aria-label="Message"
          onKeyDown={(e) => {
            // Enter sends; Shift+Enter is a new line. Not while an input method composes.
            if (e.key === "Enter" && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div class="crow">
          <button class="chip" disabled={!props.active} title="Mention the open document" onClick={addContext}>
            @ Add context
          </button>
          <span class="spacer" />
          <button class="send" title="Send ↩" onClick={send}>
            <Icon name="up" small />
          </button>
        </div>
      </div>
      <div class="cstrip">
        <button class="cs modelbtn" title="Model and thinking effort" onClick={() => setPopover(popover() === "model" ? undefined : "model")}>
          <ProviderIcon kind={props.terminal.kind} />
          <span class="cs-model">{modelLabel(props.terminal)}</span>
          <Show when={effort()}>
            <span class="eff-l">{effortName(effort()!)}</span>
          </Show>
          <Icon name="chevron" small />
        </button>
        <span class="spacer" />
        <button class="cs permbtn" title="Permissions" onClick={() => setPopover(popover() === "perm" ? undefined : "perm")}>
          <span>{perm().label}</span>
          <Icon name="chevron" small />
        </button>
      </div>
      <Show when={popover() === "model"}>
        <div class="apop">
          <Show when={waiting()}>
            <p class="ap-n">Changes here wait until you answer {kind()?.name ?? "the agent"} in the terminal.</p>
          </Show>
          <div class="ap-h">Model</div>
          <Show
            when={props.terminal.kind === "claude"}
            fallback={
              <button class="ap-opt" title="Opens Codex's /model menu in the terminal" onClick={() => command("/model")}>
                <ProviderIcon kind="codex" />
                Choose in Codex…
              </button>
            }
          >
            <For each={kind()?.models ?? []}>
              {(model) => (
                <button class="ap-opt" title={`Switches with /model ${model.id}; the conversation carries over`} onClick={() => command(`/model ${model.id}`)}>
                  <ProviderIcon kind="claude" />
                  {model.label}
                  <Show when={currentModel(props.terminal, model)}>
                    <span class="ap-ck">
                      <Icon name="check" small />
                    </span>
                  </Show>
                </button>
              )}
            </For>
          </Show>
          <Show when={kind()}>
            {(k) => (
              <EffortControl
                levels={k().efforts}
                value={effort()}
                recommended={k().recommended}
                onChange={changeEffort}
                disabled={props.terminal.kind === "claude" ? undefined : "Codex changes effort in its own /model menu"}
              />
            )}
          </Show>
        </div>
      </Show>
      <Show when={popover() === "perm"}>
        <div class="apop perm">
          <div class="ap-h">{perm().label}</div>
          <p class="ap-n">{perm().detail}</p>
          <Show when={started().id !== perm().id}>
            <p class="ap-n">Started as {started().label}.</p>
          </Show>
          <p class="ap-n">
            {props.terminal.kind === "claude" ? (
              <>
                Press <kbd>⇧Tab</kbd> in the terminal to switch modes for this session.
              </>
            ) : (
              <>
                Type <code>/approvals</code> in the terminal to change it for this session.
              </>
            )}
          </p>
        </div>
      </Show>
      <Show when={popover() === "context"}>
        <ContextPopover terminal={props.terminal} onCompact={() => command("/compact")} />
      </Show>
    </div>
  );
}

function ContextPopover(props: { terminal: TerminalInfo; onCompact: () => void }): JSX.Element {
  const status = () => props.terminal.agent?.status;
  const used = () => {
    const size = status()?.context_size;
    const percent = status()?.context ?? 0;
    return size ? `${tokens(Math.round((size * percent) / 100))} of ${tokens(size)}` : `${percent}%`;
  };
  return (
    <div class="apop">
      <div class="ap-h">
        Context <small>{used()} used</small>
      </div>
      <div class="cl-track big">
        <i style={{ width: `${Math.max(1, status()?.context ?? 0)}%` }} classList={{ warn: (status()?.context ?? 0) >= 70 }} />
      </div>
      <p class="ap-n">
        Compacting replaces the conversation with a summary, to make room. Sent to the agent as <code>/compact</code>.
      </p>
      <div class="ap-act">
        <button class="primary" onClick={props.onCompact}>
          Compact now
        </button>
      </div>
    </div>
  );
}

/** Whether `model` is the one the session runs: as the agent reports it, else as it started. */
function currentModel(terminal: TerminalInfo, model: ModelChoice): boolean {
  const reported = terminal.agent?.status?.model;
  if (reported) return reported.toLowerCase().includes(model.label.toLowerCase());
  return terminal.options?.model === model.id;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
