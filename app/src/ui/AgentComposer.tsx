// The message box under an agent's chat (draft 18): a paperclip that
// mentions the open document, the text, and a round send button that becomes
// a stop button while the agent works. Under it, the mode (a hand), the
// linked file, the context ring and the model with its thinking effort.

import { createSignal, For, type JSX, onCleanup, Show } from "solid-js";
import type { AgentStatus } from "../backend/types";
import { type AgentKind, effortName, type Mode, MODES, type ModelChoice, tokens } from "../model/agents";
import { EffortControl } from "./EffortControl";
import { FileIcon, Icon, ProviderIcon } from "./icons";

const BOX_MIN = 38;
const BOX_MAX = 420;

export interface ComposerProps {
  kind: AgentKind;
  /** The model's name: "Opus 5.5". */
  model: string;
  /** Whether `choice` is the model in use, for its check mark. */
  isModel: (choice: ModelChoice) => boolean;
  effort?: string;
  mode: Mode;
  /** Why a mode can't be picked here, if it can't. */
  modeNote?: (mode: Mode) => string | undefined;
  /** The linked file, relative to the project. */
  file?: string;
  status?: AgentStatus;
  /** The agent is working: the send button stops it, unless there is text. */
  busy?: boolean;
  /** A note over the model popup, as when changes wait for an answer. */
  modelNote?: string;
  /** Why the effort can't be changed here, if it can't. */
  effortNote?: string;
  /** What the paperclip mentions: the open document. */
  mention?: string;
  /** Sends a message; returning false keeps it in the box. */
  onSend: (text: string) => boolean | undefined;
  onStop?: () => void;
  onModel: (choice: ModelChoice) => void;
  onEffort: (level: string) => void;
  onMode: (mode: Mode) => void;
  onOpenFile?: () => void;
  onCompact?: () => void;
  ref?: (el: HTMLTextAreaElement) => void;
}

export function AgentComposer(props: ComposerProps) {
  const [height, setHeight] = createSignal(BOX_MIN);
  const [text, setText] = createSignal("");
  const [popover, setPopover] = createSignal<"model" | "mode" | "context">();
  let input!: HTMLTextAreaElement;

  const send = () => {
    const message = input.value.replace(/\s+$/, "");
    if (!message) return;
    if (props.onSend(message) === false) return;
    input.value = "";
    setText("");
  };
  const mention = () => {
    const path = props.mention;
    if (!path) return;
    const at = input.selectionStart ?? input.value.length;
    const before = input.value.slice(0, at);
    input.setRangeText(`${before && !/\s$/.test(before) ? " " : ""}@${path} `, at, input.selectionEnd ?? at, "end");
    setText(input.value);
    input.focus();
  };
  const grip = (down: PointerEvent) => {
    down.preventDefault();
    const target = down.currentTarget as HTMLElement;
    target.setPointerCapture(down.pointerId);
    const start = height();
    const move = (e: PointerEvent) => setHeight(Math.max(BOX_MIN, Math.min(BOX_MAX, start + down.clientY - e.clientY)));
    const up = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", up);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
  };
  const closePopover = (e: MouseEvent) => {
    if (!(e.target as Element).closest(".apop, .cchip, .cbi")) setPopover(undefined);
  };
  document.addEventListener("mousedown", closePopover);
  onCleanup(() => document.removeEventListener("mousedown", closePopover));
  const toggle = (which: "model" | "mode" | "context") => setPopover(popover() === which ? undefined : which);

  const stopping = () => props.busy && !text().trim() && !!props.onStop;
  const used = () => {
    const status = props.status;
    if (status?.context === undefined) return undefined;
    return status.context_size ? tokens(Math.round((status.context_size * status.context) / 100)) : `${Math.round(status.context)}%`;
  };

  return (
    <div class="composer2">
      <div class="cbox">
        <span class="grip" title="Drag to resize the message box" onPointerDown={grip} />
        <div class="cb-top">
          <button class="cbi" disabled={!props.mention} title={props.mention ? `Mention ${props.mention}` : "Open a document to mention it"} onClick={mention}>
            <Icon name="clip" />
          </button>
          <textarea
            ref={(el) => {
              input = el;
              props.ref?.(el);
            }}
            class="ain"
            rows={1}
            style={{ height: `${height()}px` }}
            placeholder={props.kind.placeholder}
            aria-label="Message"
            onInput={(e) => setText(e.currentTarget.value)}
            onKeyDown={(e) => {
              // Enter sends; Shift+Enter is a new line. Not while an input method composes.
              if (e.key === "Enter" && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
                e.preventDefault();
                send();
              }
            }}
          />
          <Show
            when={stopping()}
            fallback={
              <button class="csend" title="Send ↩" disabled={!text().trim()} onClick={send}>
                <Icon name="up" />
              </button>
            }
          >
            <button class="csend stop" title="Stop (esc in the terminal)" onClick={() => props.onStop?.()}>
              <span class="stopsq" />
            </button>
          </Show>
        </div>
        <div class="cb-bot">
          <button class="cbi mode" classList={{ named: props.mode.id !== "normal" }} title={`Mode: ${props.mode.label}. ${props.mode.detail}`} onClick={() => toggle("mode")}>
            <Icon name={props.mode.icon} />
            <Show when={props.mode.id !== "normal"}>
              <span>{props.mode.label}</span>
            </Show>
          </button>
          <span class="spacer" />
          <Show when={props.file}>
            {(file) => (
              <button class="cchip file" title={`Linked file: ${file()}. Code goes in here, below your active cell`} onClick={() => props.onOpenFile?.()}>
                <FileIcon name={file()} />
                <span>{baseName(file())}</span>
              </button>
            )}
          </Show>
          <Show when={used()}>
            <button class="cchip tok" title={`Context: ${Math.round(props.status?.context ?? 0)}% used. Click for details`} onClick={() => toggle("context")}>
              <ContextRing percent={props.status?.context ?? 0} />
              <span>{used()}</span>
            </button>
          </Show>
          <button class="cchip model" title="Model and thinking effort" onClick={() => toggle("model")}>
            <ProviderIcon kind={props.kind.kind} />
            <span>
              {props.model}
              {/* JSX, not a string, so it changes as the effort does. */}
              <Show when={props.effort ?? props.kind.recommended}>{(effort) => <> ({effortName(effort())})</>}</Show>
            </span>
            <Icon name="chevron" small />
          </button>
        </div>
      </div>
      <Show when={popover() === "mode"}>
        <div class="apop mode">
          <div class="ap-h">Mode</div>
          <For each={MODES.filter((m) => m.offered || m.id === props.mode.id)}>
            {(mode) => {
              const note = () => props.modeNote?.(mode);
              return (
                <button
                  class="ap-opt pm-opt"
                  classList={{ on: mode.id === props.mode.id }}
                  disabled={!!note()}
                  title={note()}
                  onClick={() => {
                    setPopover(undefined);
                    if (mode.id !== props.mode.id) props.onMode(mode);
                  }}
                >
                  <Icon name={mode.icon} />
                  <span>
                    <b>{mode.label}</b>
                    <small>{note() ?? mode.detail}</small>
                  </span>
                  <Show when={mode.id === props.mode.id}>
                    <span class="ap-ck">
                      <Icon name="check" small />
                    </span>
                  </Show>
                </button>
              );
            }}
          </For>
        </div>
      </Show>
      <Show when={popover() === "model"}>
        <div class="apop model">
          <Show when={props.modelNote}>
            <p class="ap-n">{props.modelNote}</p>
          </Show>
          <div class="ap-h">Model</div>
          <For each={props.kind.models}>
            {(model) => (
              <button class="ap-opt" onClick={() => props.onModel(model)}>
                <ProviderIcon kind={props.kind.kind} />
                {model.label}
                <Show when={props.isModel(model)}>
                  <span class="ap-ck">
                    <Icon name="check" small />
                  </span>
                </Show>
              </button>
            )}
          </For>
          <EffortControl
            levels={props.kind.efforts}
            value={props.effort}
            recommended={props.kind.recommended}
            onChange={props.onEffort}
            disabled={props.effortNote}
          />
        </div>
      </Show>
      <Show when={popover() === "context" && props.status}>
        {(status) => (
          <ContextPopover
            status={status()}
            onCompact={() => {
              setPopover(undefined);
              props.onCompact?.();
            }}
          />
        )}
      </Show>
    </div>
  );
}

function ContextRing(props: { percent: number }) {
  const r = 6;
  const c = 2 * Math.PI * r;
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" class="ring" aria-hidden="true">
      <circle cx="8" cy="8" r={r} class="ring-track" />
      <circle
        cx="8"
        cy="8"
        r={r}
        class="ring-val"
        classList={{ hot: props.percent >= 80 }}
        stroke-dasharray={`${(c * Math.min(100, props.percent)) / 100} ${c}`}
        transform="rotate(-90 8 8)"
      />
    </svg>
  );
}

function ContextPopover(props: { status: AgentStatus; onCompact: () => void }): JSX.Element {
  const percent = () => props.status.context ?? 0;
  const used = () => {
    const size = props.status.context_size;
    return size ? `${tokens(Math.round((size * percent()) / 100))} of ${tokens(size)}` : `${Math.round(percent())}%`;
  };
  return (
    <div class="apop context">
      <div class="ap-h">
        Context <small>{used()} used</small>
      </div>
      <div class="cl-track big">
        <i style={{ width: `${Math.max(1, percent())}%` }} classList={{ warn: percent() >= 70 }} />
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

function baseName(path: string): string {
  return path.split("/").pop() ?? path;
}
