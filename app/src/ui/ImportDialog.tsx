// Variables' Import button: a menu of ways to read a dataset into the
// session, then a dialog to pick the file, name the variable and see the
// code before it runs. The code runs in the session like any other, so it
// shows in the Console.

import { createMemo, createResource, createSignal, For, onMount, Show } from "solid-js";
import type { DirEntry, SessionInfo } from "../backend/types";
import { joinPath, parentOf } from "../model/files";
import { codePath, type ImportKind, importable, importsFor, validVariable, variableName } from "../model/imports";
import { languageLabel, languageVersion } from "../model/machine";
import type { Workspace } from "../model/workspace";
import { notify } from "./dialogs";
import { FileIcon, Icon } from "./icons";
import { closeOnOutside } from "./popup";

/** "R 4.4.1" or "Python 3.12.4". */
function sessionName(session: SessionInfo): string {
  return [languageLabel(session.language), languageVersion(session)].filter(Boolean).join(" ");
}

export function ImportButton(props: { ws: Workspace; session?: SessionInfo; folder: string }) {
  const [open, setOpen] = createSignal(false);
  const [kind, setKind] = createSignal<ImportKind>();
  let button!: HTMLButtonElement;
  let pop: HTMLDivElement | undefined;
  closeOnOutside(() => pop, () => button, () => setOpen(false));
  const kinds = () => (props.session ? importsFor(props.session.language) : []);
  const at = () => button.getBoundingClientRect();
  return (
    <>
      <button
        class="vbtn vimport"
        ref={button}
        disabled={!kinds().length}
        title={props.session ? `Import a dataset into ${sessionName(props.session)}` : "Start R or Python to import a dataset"}
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
      >
        <Icon name="table" small />
        Import
        <Icon name="chevron" small />
      </button>
      <Show when={open() && props.session}>
        {(session) => (
          <div class="kpop imppop" role="menu" aria-label="Import a dataset" ref={pop} style={{ left: `${Math.max(8, Math.min(at().left, innerWidth - 308))}px`, top: `${at().bottom + 4}px` }}>
            <div class="mm-h">Import a dataset into {sessionName(session())}</div>
            <For each={kinds()}>
              {(k) => (
                <>
                  <Show when={k.group}>
                    <div class="mm-sep" />
                  </Show>
                  <button
                    class="mm-i"
                    role="menuitem"
                    title={`Opens a preview of the ${k.fn} code it will run`}
                    onClick={() => {
                      setOpen(false);
                      setKind(k);
                    }}
                  >
                    <span />
                    <span>{k.label}</span>
                    <kbd>{k.fn}</kbd>
                  </button>
                </>
              )}
            </For>
          </div>
        )}
      </Show>
      <Show when={kind() && props.session}>
        <ImportDialog ws={props.ws} session={props.session!} kind={kind()!} folder={props.folder} onClose={() => setKind(undefined)} />
      </Show>
    </>
  );
}

/** Pick the file (or 10x folder), name the variable, see the code, run it. */
export function ImportDialog(props: { ws: Workspace; session: SessionInfo; kind: ImportKind; folder: string; onClose: () => void }) {
  const [dir, setDir] = createSignal(props.folder);
  const [file, setFile] = createSignal<string>();
  const [name, setName] = createSignal<string>();
  const [running, setRunning] = createSignal(false);
  const [entries] = createResource(dir, async (path) =>
    (await props.ws.call<DirEntry[]>("list_dir", { path })).filter(
      (e) => !e.name.startsWith(".") && (e.kind === "dir" || (e.kind === "file" && importable(props.kind, e.name))),
    ),
  );
  /** What it reads: the picked file, or for 10x output the folder shown. */
  const chosen = () => file() ?? (props.kind.folder ? dir() : undefined);
  const variable = () => name() ?? (chosen() ? variableName(chosen()!) : "");
  const named = () => !chosen() || props.kind.named(chosen()!);
  const badName = () => named() && !!variable() && !validVariable(variable(), props.session.language);
  const code = createMemo(() => {
    const path = chosen();
    return path ? props.kind.code(codePath(path, props.session.cwd), variable() || "data") : "";
  });
  const go = (path: string) => {
    setDir(path);
    setFile(undefined);
  };
  async function run() {
    if (!code() || badName()) return;
    setRunning(true);
    try {
      await props.ws.execute(props.session.id, code());
      props.onClose();
    } catch (err) {
      notify(`Could not import: ${err instanceof Error ? err.message : String(err)}`);
      setRunning(false);
    }
  }
  let nameField: HTMLInputElement | undefined;
  onMount(() => nameField?.focus());
  return (
    <div class="modal-backdrop" onKeyDown={(e) => e.key === "Escape" && props.onClose()}>
      <div class="modal importdlg" role="dialog" aria-modal="true" aria-label={`Import ${props.kind.label}`}>
        <p class="modal-message">
          {props.kind.label.replace(/…$/, "")} into {sessionName(props.session)}
        </p>
        <div class="imp-browser">
          <div class="imp-path">
            <button title="Parent folder" disabled={dir() === "/"} onClick={() => go(parentOf(dir()))}>
              <Icon name="up" small />
            </button>
            <span title={dir()}>{dir()}</span>
          </div>
          <ul class="imp-list">
            <Show when={!entries.error} fallback={<li class="muted">{String(entries.error)}</li>}>
              <For each={entries()} fallback={<li class="muted">{entries.loading ? "Loading…" : props.kind.folder ? "No folders or .h5 files here" : "No files it reads here"}</li>}>
                {(entry) => {
                  const path = joinPath(dir(), entry.name);
                  return (
                    <li>
                      <button
                        classList={{ on: file() === path }}
                        onClick={() => (entry.kind === "dir" ? go(path) : setFile(path))}
                        onDblClick={() => entry.kind === "file" && void run()}
                      >
                        <FileIcon name={entry.name} dir={entry.kind === "dir"} />
                        {entry.name}
                      </button>
                    </li>
                  );
                }}
              </For>
            </Show>
          </ul>
        </div>
        <Show when={named()}>
          <label class="imp-name">
            <span>Name</span>
            <input
              ref={nameField}
              value={variable()}
              spellcheck={false}
              autocomplete="off"
              aria-invalid={badName()}
              onInput={(e) => setName(e.currentTarget.value)}
              onKeyDown={(e) => e.key === "Enter" && void run()}
            />
          </label>
          <Show when={badName()}>
            <p class="tnew-err">That isn't a {languageLabel(props.session.language)} variable name</p>
          </Show>
        </Show>
        <pre class="imp-code" aria-label="Code it will run">{code() || (props.kind.folder ? "Open the folder with the 10x output, or pick an .h5 file" : "Pick a file")}</pre>
        <div class="modal-buttons">
          <button onClick={props.onClose}>Cancel</button>
          <button class="primary" disabled={!code() || badName() || running()} onClick={() => void run()}>
            Import
          </button>
        </div>
      </div>
    </div>
  );
}
