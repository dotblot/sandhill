import { createResource, createSignal, For, Show } from "solid-js";
import { LOCAL_HOST } from "../backend";
import type { SessionInfo, SystemStats } from "../backend/types";
import type { Doc } from "../docs/doc";
import { formatBytes, languageLabel, languageVersion, sessionLabel } from "../model/machine";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { notify } from "./dialogs";
import { Icon, LangIcon } from "./icons";
import { closeOnOutside } from "./popup";

/**
 * One kernel picker per language in the document: the language's icon and
 * version, with interrupt and restart; its drop-down holds the details.
 */
export function KernelBar(props: {
  ws: Workspace;
  doc: Doc;
  /** The names of the open documents using a session. */
  usedBy?: (session: string) => string[];
  onError: (msg: string) => void;
}) {
  const languages = () => props.doc.languages();
  // Shows every output when none are shown, else hides them all.
  const anyShown = () => props.doc.shownOutputs().length > 0;
  const runAll = () => props.doc.runAll().catch((e) => props.onError(String(e)));
  return (
    <div class="kernel-bar">
      <For each={languages()} fallback={<span class="kernel-none">No R or Python code yet</span>}>
        {(language) => (
          <KernelControl ws={props.ws} doc={props.doc} language={language} usedBy={props.usedBy} onError={props.onError} />
        )}
      </For>
      <span class="spacer" />
      <Show
        when={props.doc.kind === "script"}
        fallback={
          <>
            <button
              class="bar-button outputs-toggle"
              onClick={() => props.doc.setAllOutputs(!anyShown())}
              title={anyShown() ? "Hide every output in this document" : "Show every output in this document"}
            >
              <Icon name={anyShown() ? "chevron" : "chevronRight"} small />
              {anyShown() ? "Hide outputs" : "Show outputs"}
            </button>
            <button class="bar-button" onClick={runAll} title="Run every chunk, top to bottom (⌥⌘R)">
              <Icon name="play" small />
              Run all
            </button>
          </>
        }
      >
        <button
          class="bar-button run-line"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => props.doc.runCurrent?.()}
          title="Run the selection, or the line or statement at the cursor, and move to the next"
        >
          <Icon name="play" small />
          Run line <kbd>⌘↵</kbd>
        </button>
        <button class="bar-button run-file" onClick={runAll} title="Run the whole file (also ⇧⌘S and ⌥⌘R)">
          <Icon name="play" small />
          Run file <kbd>⇧⌘↵</kbd>
        </button>
      </Show>
    </div>
  );
}

function KernelControl(props: {
  ws: Workspace;
  doc: Doc;
  language: string;
  usedBy?: (session: string) => string[];
  onError: (msg: string) => void;
}) {
  const [open, setOpen] = createSignal<{ left: number; top: number }>();
  const session = () => props.doc.kernels.session(props.language);
  const info = () => props.ws.sessions().find((s) => s.id === session());
  const state = () => kernelState(props.ws, props.doc, props.language);
  const label = () => languageLabel(props.language);
  const version = () => (info() ? languageVersion(info()!) : "");
  const guard = (p: Promise<unknown>) => p.catch((e) => props.onError(String(e)));
  let button!: HTMLButtonElement;

  const toggle = () => {
    if (open()) return setOpen(undefined);
    const box = button.getBoundingClientRect();
    setOpen({ left: Math.max(8, Math.min(box.left - 4, window.innerWidth - 278)), top: box.bottom + 4 });
  };

  return (
    <div class="kernel" data-state={state()}>
      <LangIcon language={props.language} />
      <button
        ref={button}
        class="kernel-pick"
        aria-haspopup="menu"
        aria-expanded={!!open()}
        title={`${[label(), version()].filter(Boolean).join(" ")} · ${STATE_TEXT[state()]}. Click for details`}
        onClick={toggle}
      >
        <span classList={{ "kernel-off": !info() }}>{info() ? version() || label() : state() === "starting" ? "starting…" : "not started"}</span>
        <Icon name="chevron" small />
      </button>
      <button
        class="kernel-button"
        disabled={state() !== "busy"}
        onClick={() => guard(props.doc.kernels.interrupt(props.language))}
        title="Interrupt"
      >
        <Icon name="stop" small />
      </button>
      <button
        class="kernel-button"
        disabled={!session()}
        onClick={() => guard(props.doc.kernels.restart(props.language))}
        title="Restart"
      >
        <Icon name="refresh" small />
      </button>
      <Show when={open()}>
        {(at) => (
          <KernelMenu
            ws={props.ws}
            doc={props.doc}
            language={props.language}
            usedBy={props.usedBy}
            at={at()}
            anchor={button}
            onClose={() => setOpen(undefined)}
            onError={props.onError}
          />
        )}
      </Show>
    </div>
  );
}

type KernelState = "starting" | "none" | "busy" | "idle";

const STATE_TEXT: Record<KernelState, string> = {
  starting: "starting",
  none: "starts when you run code",
  busy: "busy",
  idle: "idle",
};

export function kernelState(ws: Workspace, doc: Doc, language: string): KernelState {
  if (doc.kernels.isStarting(language)) return "starting";
  const id = doc.kernels.session(language);
  if (!id) return "none";
  return ws.isBusy(id) ? "busy" : "idle";
}

/**
 * A kernel picker's drop-down: the session's language, version and state,
 * its kernel, environment, machine, documents and memory, then the session
 * list: the current one, others in the project, and new sessions of each
 * installed kernel for the language. Session IDs appear only here.
 */
/**
 * A left-pane kernel row's pop-up, beside the row: the same details as a
 * kernel picker's, then what can be done to the session from there.
 */
export function SessionMenu(props: {
  ws: Workspace;
  session: SessionInfo;
  /** The documents using it. */
  usedBy: string[];
  anchor: HTMLElement;
  onClose: () => void;
  onShutDown: () => void;
}) {
  const ws = props.ws;
  const state = () => (ws.isBusy(props.session.id) ? "busy" : "idle");
  const memory = sessionMemory(ws, () => props.session.id);
  const rows = () => [...sessionRows(ws, props.session, props.usedBy, memory()), ["Session ID", props.session.id.slice(0, 8)] as [string, string]];
  const box = props.anchor.getBoundingClientRect();
  const [top, setTop] = createSignal(box.top - 6);
  let menu!: HTMLDivElement;
  closeOnOutside(() => menu, props.anchor, props.onClose);
  // Beside the row, moved up as far as it must to stay in the window.
  queueMicrotask(() => setTop(Math.max(8, Math.min(box.top - 6, window.innerHeight - menu.offsetHeight - 8))));
  const interrupt = () => {
    props.onClose();
    props.ws.interrupt(props.session.id).catch((e) => notify(`Could not interrupt ${sessionLabel(props.session)}: ${String(e)}`));
  };
  return (
    <div ref={menu} class="kpop" role="menu" style={{ left: `${box.right + 8}px`, top: `${top()}px` }}>
      <KernelHead language={props.session.language} name={sessionLabel(props.session)} state={state()} rows={rows()} />
      <div class="mm-sep" />
      <div class="mm-h">Session</div>
      <button class="mm-i" role="menuitem" disabled={state() !== "busy"} onClick={interrupt}>
        <Icon name="stop" small />
        <span>Interrupt</span>
        <kbd />
      </button>
      <button
        class="mm-i"
        role="menuitem"
        onClick={() => {
          props.onShutDown();
          props.onClose();
        }}
      >
        <Icon name="trash" small />
        <span>Shut down…</span>
        <kbd />
      </button>
    </div>
  );
}

/**
 * A session picker, as in Variables: the language icon and version, like
 * the editor's kernel pickers, with the same details in its drop-down and
 * the project's sessions to pick from.
 */
export function SessionPicker(props: {
  ws: Workspace;
  session?: SessionInfo;
  sessions: SessionInfo[];
  usedBy?: (session: string) => string[];
  onPick: (session: string) => void;
}) {
  const [open, setOpen] = createSignal(false);
  const state = (): KernelState => (props.session ? (props.ws.isBusy(props.session.id) ? "busy" : "idle") : "none");
  let button!: HTMLButtonElement;
  return (
    <div class="kernel session-pick" data-state={state()}>
      <Show when={props.session} fallback={<span class="kernel-none">No R or Python session</span>}>
        {(s) => (
          <>
            <LangIcon language={s().language} />
            <button
              ref={button}
              class="kernel-pick"
              aria-haspopup="menu"
              aria-expanded={open()}
              title={`${sessionLabel(s())} · ${STATE_TEXT[state()]}. Click for details`}
              onClick={() => setOpen(!open())}
            >
              <span>{languageVersion(s()) || languageLabel(s().language)}</span>
              <Icon name="chevron" small />
            </button>
          </>
        )}
      </Show>
      <Show when={open() && props.session}>
        {(s) => (
          <SessionPickMenu
            ws={props.ws}
            session={s()}
            sessions={props.sessions}
            usedBy={props.usedBy}
            anchor={button}
            onPick={(id) => {
              setOpen(false);
              props.onPick(id);
            }}
            onClose={() => setOpen(false)}
          />
        )}
      </Show>
    </div>
  );
}

function SessionPickMenu(props: {
  ws: Workspace;
  session: SessionInfo;
  sessions: SessionInfo[];
  usedBy?: (session: string) => string[];
  anchor: HTMLElement;
  onPick: (session: string) => void;
  onClose: () => void;
}) {
  const ws = props.ws;
  const state = (): KernelState => (ws.isBusy(props.session.id) ? "busy" : "idle");
  const memory = sessionMemory(ws, () => props.session.id);
  const rows = () => sessionRows(ws, props.session, props.usedBy?.(props.session.id) ?? [], memory());
  const box = props.anchor.getBoundingClientRect();
  let menu!: HTMLDivElement;
  closeOnOutside(() => menu, props.anchor, props.onClose);
  return (
    <div
      ref={menu}
      class="kpop"
      role="menu"
      style={{ left: `${Math.max(8, Math.min(box.left - 4, window.innerWidth - 278))}px`, top: `${box.bottom + 4}px` }}
    >
      <KernelHead language={props.session.language} name={sessionLabel(props.session)} state={state()} rows={rows()} />
      <div class="mm-sep" />
      <div class="mm-h">Sessions</div>
      <For each={props.sessions}>
        {(s) => (
          <button class="mm-i" role="menuitem" onClick={() => props.onPick(s.id)}>
            {s.id === props.session.id ? <Icon name="check" small /> : <span class="ico sm" />}
            <span>
              {sessionLabel(s)} · {s.id.slice(0, 8)}
            </span>
            <kbd>{ws.isBusy(s.id) ? "busy" : ""}</kbd>
          </button>
        )}
      </For>
    </div>
  );
}

/** A kernel pop-up's top: the language, name and state, then the details. */
function KernelHead(props: { language: string; name: string; state: KernelState; rows: [string, string][] }) {
  return (
    <>
      <div class="kp-h">
        <LangIcon language={props.language} />
        <b>{props.name}</b>
        <span class="kp-st" data-state={props.state}>
          <span class="kernel-dot" />
          {STATE_TEXT[props.state]}
        </span>
      </div>
      <Show when={props.rows.length}>
        <dl class="kp-d">
          <For each={props.rows}>
            {([name, value]) => (
              <>
                <dt>{name}</dt>
                <dd title={value}>{value}</dd>
              </>
            )}
          </For>
        </dl>
      </Show>
    </>
  );
}

/** A session's kernel, environment, machine, documents and memory. */
function sessionRows(ws: Workspace, s: SessionInfo, usedBy: string[], memory?: number): [string, string][] {
  const spec = ws.kernelspecs().find((k) => k.name === s.kernel);
  const rows: [string, string][] = [["Kernel", spec?.display_name ?? s.kernel]];
  if (spec?.env) rows.push(["Environment", spec.env]);
  rows.push(["Runs on", ws.backend.host === LOCAL_HOST ? "This Mac" : ws.backend.host]);
  if (usedBy.length) rows.push(["Used by", usedBy.join(", ")]);
  if (memory !== undefined) rows.push(["Memory", formatBytes(memory)]);
  return rows;
}

/** The memory a session's kernel holds, asked for once. */
function sessionMemory(ws: Workspace, session: () => string | undefined) {
  const [memory] = createResource(session, async (id) => {
    const stats = await ws.call<SystemStats>("system_stats", {}).catch(() => undefined);
    return stats?.sessions.find((m) => m.session === id)?.rss;
  });
  return memory;
}

function KernelMenu(props: {
  ws: Workspace;
  doc: Doc;
  language: string;
  usedBy?: (session: string) => string[];
  at: { left: number; top: number };
  anchor: HTMLElement;
  onClose: () => void;
  onError: (msg: string) => void;
}) {
  const ws = props.ws;
  const session = () => props.doc.kernels.session(props.language);
  const info = () => ws.sessions().find((s) => s.id === session());
  const state = () => kernelState(ws, props.doc, props.language);
  const specs = () => ws.kernelspecs().filter((k) => normalizeLanguage(k.language) === props.language);
  const others = () => ws.projectSessions(props.language).filter((s) => s.id !== session());
  const memory = sessionMemory(ws, session);
  const rows = () => (info() ? sessionRows(ws, info()!, props.usedBy?.(info()!.id) ?? [], memory()) : []);
  const run = (action: () => unknown) => {
    props.onClose();
    void (async () => action())().catch((e) => props.onError(String(e)));
  };
  let menu!: HTMLDivElement;
  closeOnOutside(() => menu, props.anchor, props.onClose);

  return (
    <div ref={menu} class="kpop" role="menu" style={{ left: `${props.at.left}px`, top: `${props.at.top}px` }}>
      <KernelHead
        language={props.language}
        name={info() ? sessionLabel(info()!) : languageLabel(props.language)}
        state={state()}
        rows={rows()}
      />
      <div class="mm-sep" />
      <div class="mm-h">Session</div>
      <Show when={info()}>
        {(s) => (
          <button class="mm-i" role="menuitem" onClick={props.onClose}>
            <Icon name="check" small />
            <span>{s().id.slice(0, 8)}</span>
            <kbd>current</kbd>
          </button>
        )}
      </Show>
      <For each={others()}>
        {(s) => (
          <button class="mm-i" role="menuitem" onClick={() => run(() => props.doc.kernels.assign(props.language, s.id))}>
            <span class="ico sm" />
            <span>
              Use {sessionLabel(s)} · {s.id.slice(0, 8)}
            </span>
            <kbd>{ws.isBusy(s.id) ? "busy" : ""}</kbd>
          </button>
        )}
      </For>
      <For each={specs()}>
        {(k) => (
          <button class="mm-i" role="menuitem" onClick={() => run(() => props.doc.kernels.startNew(props.language, k.name))}>
            {k.name === info()?.kernel || (!info() && k === specs()[0]) ? <Icon name="plus" small /> : <span class="ico sm" />}
            <span>
              {k.name === info()?.kernel
                ? `New ${sessionLabel(info()!)} session`
                : info()
                  ? `Switch to ${k.display_name}`
                  : `Start ${k.display_name}`}
            </span>
            <kbd />
          </button>
        )}
      </For>
    </div>
  );
}
