import { createEffect, For, Show } from "solid-js";
import type { SessionInfo } from "../backend/types";
import type { Doc } from "../docs/doc";
import { kernelShortName, languageLabel, languageVersion } from "../model/machine";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { Icon } from "./icons";

/** One control per language in the document: which session runs it, and its state. */
export function KernelBar(props: { ws: Workspace; doc: Doc; onError: (msg: string) => void }) {
  const languages = () => props.doc.languages();
  // Shows every output when none are shown, else hides them all.
  const anyShown = () => props.doc.shownOutputs().length > 0;
  const runAll = () => props.doc.runAll().catch((e) => props.onError(String(e)));
  return (
    <div class="kernel-bar">
      <For each={languages()} fallback={<span class="kernel-none">No R or Python code yet</span>}>
        {(language) => <KernelControl ws={props.ws} doc={props.doc} language={language} onError={props.onError} />}
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
            <button class="bar-button" onClick={runAll} title="Run every chunk, top to bottom">
              <Icon name="play" small />
              Run all <kbd>⌥⌘R</kbd>
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

function KernelControl(props: { ws: Workspace; doc: Doc; language: string; onError: (msg: string) => void }) {
  const session = () => props.doc.kernels.session(props.language);
  const info = () => props.ws.sessions().find((s) => s.id === session());
  const state = () => {
    if (props.doc.kernels.isStarting(props.language)) return "starting";
    const id = session();
    if (!id) return "none";
    return props.ws.isBusy(id) ? "busy" : "idle";
  };
  const label = () => languageLabel(props.language);
  /** "4.4.1 · Ark · 7f3a12": the version, the kernel and the session. */
  const describe = (s: SessionInfo) =>
    [languageVersion(s), kernelShortName(s.kernel, props.ws.kernelspecs()), s.id.slice(0, 6)]
      // IRkernel is just "R", which the pill already says.
      .filter((part, i) => part && !(i === 1 && part === label()))
      .join(" · ");
  const specs = () => props.ws.kernelspecs().filter((k) => normalizeLanguage(k.language) === props.language);
  const others = () => props.ws.projectSessions(props.language).filter((s) => s.id !== session());
  const guard = (p: Promise<unknown>) => p.catch((e) => props.onError(String(e)));

  // Set the choice once the options for it are in place. Through the
  // `value` attribute it can land while the options are still changing (as
  // when a tab closes), and WebKit then shows an empty picker.
  let select!: HTMLSelectElement;
  createEffect(() => {
    const value = session() ? `session:${session()}` : "";
    info();
    others();
    specs();
    state();
    select.value = value;
  });

  const onSelect = (value: string) => {
    if (value.startsWith("session:")) props.doc.kernels.assign(props.language, value.slice(8));
    else if (value.startsWith("new:")) void guard(props.doc.kernels.startNew(props.language, value.slice(4)));
  };

  return (
    <div class="kernel" data-state={state()}>
      <span class="kernel-dot" title={state()} />
      <span class="kernel-lang">{label()}</span>
      <span class="kernel-pick">
        <select ref={select} onChange={(e) => onSelect(e.currentTarget.value)} title="Which session runs this language">
          <option value="" disabled>
            {state() === "starting" ? "Starting…" : "Starts when you run code"}
          </option>
          <Show when={info()}>
            {(s) => (
              <option value={`session:${s().id}`}>{describe(s())}</option>
            )}
          </Show>
          <For each={others()}>
            {(s) => (
              <option value={`session:${s.id}`}>Use {describe(s)}</option>
            )}
          </For>
          <For each={specs()}>{(k) => <option value={`new:${k.name}`}>New {k.display_name}</option>}</For>
        </select>
        <Icon name="chevron" small />
      </span>
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
    </div>
  );
}
