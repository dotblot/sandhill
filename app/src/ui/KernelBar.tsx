import { createEffect, For, Show } from "solid-js";
import type { Doc } from "../docs/doc";
import { normalizeLanguage, type Workspace } from "../model/workspace";

/** One control per language in the document: which session runs it, and its state. */
export function KernelBar(props: { ws: Workspace; doc: Doc; onError: (msg: string) => void }) {
  const languages = () => props.doc.languages();
  return (
    <div class="kernel-bar">
      <For each={languages()} fallback={<span class="muted">No R or Python code yet</span>}>
        {(language) => <KernelControl ws={props.ws} doc={props.doc} language={language} onError={props.onError} />}
      </For>
      <span class="spacer" />
      <button onClick={() => props.doc.runAll().catch((e) => props.onError(String(e)))} title="Run all (⌥⌘R)">
        Run all
      </button>
      <button
        onClick={() => props.doc.save().catch((e) => props.onError(String(e)))}
        disabled={!props.doc.dirty()}
        title="Save (⌘S)"
      >
        Save
      </button>
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
  const label = () => (props.language === "r" ? "R" : props.language === "python" ? "Python" : props.language);
  const specs = () => props.ws.kernelspecs().filter((k) => normalizeLanguage(k.language) === props.language);
  const others = () => props.ws.liveSessions(props.language).filter((s) => s.id !== session());
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
      <select ref={select} onChange={(e) => onSelect(e.currentTarget.value)}>
        <option value="" disabled>
          {state() === "starting" ? "Starting…" : "Starts when you run code"}
        </option>
        <Show when={info()}>
          {(s) => (
            <option value={`session:${s().id}`}>
              {s().kernel} · {s().id.slice(0, 6)}
            </option>
          )}
        </Show>
        <For each={others()}>
          {(s) => (
            <option value={`session:${s.id}`}>
              Use {s.kernel} · {s.id.slice(0, 6)}
            </option>
          )}
        </For>
        <For each={specs()}>{(k) => <option value={`new:${k.name}`}>New {k.display_name}</option>}</For>
      </select>
      <button disabled={state() !== "busy"} onClick={() => guard(props.doc.kernels.interrupt(props.language))} title="Interrupt">
        ■
      </button>
      <button disabled={!session()} onClick={() => guard(props.doc.kernels.restart(props.language))} title="Restart">
        ↻
      </button>
    </div>
  );
}
