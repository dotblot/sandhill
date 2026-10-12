import { createResource, createSignal, For, onCleanup, Show } from "solid-js";
import { LOCAL_HOST } from "../backend";
import type { SystemStats } from "../backend/types";
import type { Doc } from "../docs/doc";
import { formatBytes, languageLabel, languageVersion, sessionLabel } from "../model/machine";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { Icon, LangIcon } from "./icons";

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
  const spec = () => ws.kernelspecs().find((k) => k.name === info()?.kernel);
  const others = () => ws.projectSessions(props.language).filter((s) => s.id !== session());
  const [memory] = createResource(session, async (id) => {
    const stats = await ws.call<SystemStats>("system_stats", {}).catch(() => undefined);
    return stats?.sessions.find((m) => m.session === id)?.rss;
  });
  const host = ws.backend.host === LOCAL_HOST ? "This Mac" : ws.backend.host;
  const rows = (): [string, string][] => {
    const s = info();
    if (!s) return [];
    const list: [string, string][] = [["Kernel", spec()?.display_name ?? s.kernel]];
    if (spec()?.env) list.push(["Environment", spec()!.env!]);
    list.push(["Runs on", host]);
    const docs = props.usedBy?.(s.id) ?? [];
    if (docs.length) list.push(["Used by", docs.join(", ")]);
    if (memory() !== undefined) list.push(["Memory", formatBytes(memory()!)]);
    return list;
  };
  const run = (action: () => unknown) => {
    props.onClose();
    void (async () => action())().catch((e) => props.onError(String(e)));
  };

  let menu!: HTMLDivElement;
  const outside = (e: MouseEvent) => {
    const target = e.target as Node;
    if (!menu.contains(target) && !props.anchor.contains(target)) props.onClose();
  };
  const escape = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
  document.addEventListener("mousedown", outside);
  document.addEventListener("keydown", escape);
  onCleanup(() => {
    document.removeEventListener("mousedown", outside);
    document.removeEventListener("keydown", escape);
  });

  return (
    <div ref={menu} class="kpop" role="menu" style={{ left: `${props.at.left}px`, top: `${props.at.top}px` }}>
      <div class="kp-h">
        <LangIcon language={props.language} />
        <b>{info() ? sessionLabel(info()!) : languageLabel(props.language)}</b>
        <span class="kp-st" data-state={state()}>
          <span class="kernel-dot" />
          {STATE_TEXT[state()]}
        </span>
      </div>
      <Show when={rows().length}>
        <dl class="kp-d">
          <For each={rows()}>
            {([name, value]) => (
              <>
                <dt>{name}</dt>
                <dd title={value}>{value}</dd>
              </>
            )}
          </For>
        </dl>
      </Show>
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
