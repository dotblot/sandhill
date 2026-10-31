// The message box under an agent's chat (drafts 18 and 30–35): chips for
// what was added with @, the text, and a round send button that becomes a
// stop button while the agent works. Under it, on the left: web search,
// session context and the mode (a hand); on the right: the context ring and
// the model with its thinking effort. Conversations are about the whole
// project: files are added with @, which shows no button of its own.

import { createEffect, createSignal, For, type JSX, on, onCleanup, Show } from "solid-js";
import type { AgentStatus, MessageRef } from "../backend/types";
import {
  type AgentKind,
  type CodeTo,
  effortName,
  favourites,
  hasMode,
  type Mode,
  MODES,
  type ModelChoice,
  modelKey,
  saveFavourites,
  shortModel,
  tokens,
} from "../model/agents";
import { filterMentions, type Mention, sameRef, typedMention } from "../model/mentions";
import { notify } from "./dialogs";
import { EffortControl } from "./EffortControl";
import { FileIcon, Icon, ProviderIcon } from "./icons";

const BOX_MIN = 38;
const BOX_MAX = 420;
const CODE_TO: [CodeTo, string, string][] = [
  ["file", "In the file", "The agent adds its code to the focus file, below your active cell, and runs it there"],
  ["console", "In the Console", "The agent runs its code in the Console, and the focus file stays as it is"],
];

/** Unsent text and chips, by conversation, so they stay when the pane redraws or another conversation is shown. */
const drafts = new Map<string, { text: string; refs: MessageRef[] }>();

type Popover = "model" | "mode" | "context" | "mention";

export interface ComposerProps {
  /** The conversation the box is for ("new" for a new one): its draft is kept under this. */
  draftKey: string;
  kind: AgentKind;
  /** Every agent, for the model menu's tabs; `kind` among them. */
  kinds: AgentKind[];
  /** The conversation has started, so its agent can't change: other agents' models are dimmed. */
  locked?: boolean;
  /** The model's name: "Opus 5.5". */
  model: string;
  /** Whether `choice` of `kind` is the model in use, for the shaded row. */
  isModel: (kind: string, choice: ModelChoice) => boolean;
  effort?: string;
  mode: Mode;
  /** Why a mode can't be picked here, if it can't. */
  modeNote?: (mode: Mode) => string | undefined;
  web: boolean;
  onWeb: (on: boolean) => void;
  /** Session context: where the user is goes with every message. */
  context: boolean;
  onContext: (on: boolean) => void;
  /** What the @ menu offers, read as it opens. */
  mentions: () => Promise<Mention[]>;
  /** Saves a file from this Mac (picked, pasted or dragged) where the agent can read it. */
  onAttach: (file: File) => Promise<MessageRef>;
  status?: AgentStatus;
  /** The agent is working: the send button stops it, unless there is text. */
  busy?: boolean;
  /** A note over the model menu, as when changes wait for an answer. */
  modelNote?: string;
  /** Why the effort can't be changed here, if it can't. */
  effortNote?: string;
  /** Sends a message with its chips; returning false keeps both in the box. */
  onSend: (text: string, refs: MessageRef[]) => boolean | undefined;
  onStop?: () => void;
  onModel: (kind: AgentKind, choice: ModelChoice) => void;
  onEffort: (level: string) => void;
  onMode: (mode: Mode) => void;
  /** Where the agent's code goes, switched under the modes. */
  codeTo?: CodeTo;
  onCodeTo?: (to: CodeTo) => void;
  onCompact?: () => void;
  ref?: (el: HTMLTextAreaElement) => void;
  /** A message put in the box from elsewhere, as by an error's Fix button, to send or change; `onFilled` says it's in. */
  fill?: { text: string; refs: MessageRef[] };
  onFilled?: () => void;
}

export function AgentComposer(props: ComposerProps) {
  const [height, setHeight] = createSignal(BOX_MIN);
  const [text, setText] = createSignal("");
  const [refs, setRefs] = createSignal<MessageRef[]>([]);
  const [popover, setPopover] = createSignal<Popover>();
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  const [mentions, setMentions] = createSignal<Mention[]>();
  let input!: HTMLTextAreaElement;
  let files!: HTMLInputElement;

  /**
   * The draft's key as last read. Kept here for the cleanup: the box goes
   * when its conversation does, and reading the prop then reads a
   * conversation that is gone, which Solid refuses with an error that
   * left the whole pane stuck.
   */
  let draftKey: string | undefined;
  // Each conversation keeps its own unsent text and chips.
  createEffect(
    on(
      () => props.draftKey,
      (key, previous) => {
        draftKey = key;
        if (key === previous) return;
        if (previous !== undefined) keep(previous);
        const draft = drafts.get(key);
        input.value = draft?.text ?? "";
        setText(input.value);
        setRefs(draft?.refs ?? []);
        setPopover(undefined);
      },
    ),
  );
  createEffect(
    on(
      () => props.fill,
      (fill) => {
        if (!fill) return;
        input.value = fill.text;
        setText(fill.text);
        setRefs(fill.refs);
        setPopover(undefined);
        input.focus();
        // From its start, which says what is asked.
        input.setSelectionRange(0, 0);
        input.scrollTop = 0;
        props.onFilled?.();
      },
    ),
  );
  onCleanup(() => draftKey !== undefined && keep(draftKey));
  function keep(key: string) {
    if (input.value || refs().length) drafts.set(key, { text: input.value, refs: refs() });
    else drafts.delete(key);
  }

  const send = () => {
    const message = input.value.replace(/\s+$/, "");
    if (!message) return;
    if (props.onSend(message, refs()) === false) return;
    input.value = "";
    setText("");
    setRefs([]);
    drafts.delete(props.draftKey);
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
  const close = () => {
    if (popover() === "mention") closeMention();
    else setPopover(undefined);
  };
  const closeOutside = (e: MouseEvent) => {
    if (popover() && !(e.target as Element).closest(".apop, .cchip, .cbi, .atfile")) close();
  };
  document.addEventListener("mousedown", closeOutside);
  onCleanup(() => document.removeEventListener("mousedown", closeOutside));
  const toggle = (which: Popover) => (popover() === which ? close() : (close(), setPopover(which)));

  // The @ menu.
  const groups = () => filterMentions(mentions() ?? [], query());
  const shown = () => groups().flatMap(([, list]) => list);
  function openMention() {
    if (popover() !== "mention") {
      setPopover(undefined);
      setMentions(undefined);
      props
        .mentions()
        .then(setMentions)
        .catch(() => setMentions([]));
    }
    setActive(0);
    setPopover("mention");
  }
  /** The "@" typed to call the menu, and the letters after it, go when it closes. */
  function dropTyped() {
    const caret = input.selectionStart ?? input.value.length;
    const typed = typedMention(input.value, caret);
    if (!typed) return;
    input.setRangeText("", typed.at, caret, "end");
    setText(input.value);
  }
  function closeMention() {
    dropTyped();
    setPopover(undefined);
    setQuery("");
    input.focus();
  }
  const addRef = (ref: MessageRef) => setRefs((all) => (all.some((r) => sameRef(r, ref)) ? all : [...all, ref]));
  async function pick(mention: Mention | undefined) {
    if (!mention) return;
    closeMention();
    try {
      addRef(mention.make ? await mention.make() : mention.ref);
    } catch (err) {
      notify(`Could not add ${mention.ref.name}: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  async function attach(list: File[]) {
    for (const file of list) {
      try {
        addRef(await props.onAttach(file));
      } catch (err) {
        notify(`Could not attach ${file.name}: ${err instanceof Error ? err.message : String(err)}`);
      }
    }
  }
  /** Enter or Tab picks, arrows move, Esc closes; true when the key was the menu's. */
  function menuKey(e: KeyboardEvent): boolean {
    if (popover() !== "mention") return false;
    if ((e.key === "Enter" || e.key === "Tab") && shown().length) {
      e.preventDefault();
      void pick(shown()[active()]);
      return true;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeMention();
      return true;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = shown().length;
      if (n) setActive((i) => (i + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
      return true;
    }
    return false;
  }
  const onInput = () => {
    setText(input.value);
    const typed = typedMention(input.value, input.selectionStart ?? input.value.length);
    if (typed) {
      openMention();
      setQuery(typed.query);
    } else if (popover() === "mention") {
      setPopover(undefined);
      setQuery("");
    }
  };

  // ⌥1, ⌥2… pick a model in the open tab of the model menu.
  const [tab, setTab] = createSignal<string>();
  const [stars, setStars] = createSignal(favourites());
  const usable = (kind: AgentKind) => !props.locked || kind.kind === props.kind.kind;
  const openTab = () => {
    const t = tab() ?? props.kind.kind;
    const kind = props.kinds.find((k) => k.kind === t);
    return t === "fav" || (kind && usable(kind)) ? t : props.kind.kind;
  };
  const rows = (): { kind: AgentKind; model: ModelChoice }[] => {
    if (openTab() === "fav") {
      return props.kinds.flatMap((kind) => kind.models.filter((m) => stars().includes(modelKey(kind.kind, m))).map((model) => ({ kind, model })));
    }
    const kind = props.kinds.find((k) => k.kind === openTab()) ?? props.kind;
    return kind.models.map((model) => ({ kind, model }));
  };
  const star = (kind: AgentKind, model: ModelChoice) => {
    const key = modelKey(kind.kind, model);
    const next = stars().includes(key) ? stars().filter((k) => k !== key) : [...stars(), key];
    setStars(next);
    saveFavourites(next);
  };
  const choose = (row: { kind: AgentKind; model: ModelChoice } | undefined) => {
    if (row && usable(row.kind)) props.onModel(row.kind, row.model);
  };
  const altDigit = (e: KeyboardEvent) => {
    if (popover() !== "model" || !e.altKey || !/^Digit[1-9]$/.test(e.code)) return;
    e.preventDefault();
    choose(rows()[Number(e.code.slice(5)) - 1]);
  };
  document.addEventListener("keydown", altDigit);
  onCleanup(() => document.removeEventListener("keydown", altDigit));

  const stopping = () => props.busy && !text().trim() && !!props.onStop;
  const used = () => {
    const status = props.status;
    if (status?.context === undefined) return undefined;
    return status.context_size ? tokens(Math.round((status.context_size * status.context) / 100)) : `${Math.round(status.context)}%`;
  };

  return (
    <div class="composer2">
      <div
        class="cbox"
        onDragOver={(e) => {
          if (e.dataTransfer?.types.includes("Files")) e.preventDefault();
        }}
        onDrop={(e) => {
          const list = [...(e.dataTransfer?.files ?? [])];
          if (!list.length) return;
          e.preventDefault();
          void attach(list);
        }}
      >
        <span class="grip" title="Drag to resize the message box" onPointerDown={grip} />
        <Show when={refs().length}>
          <div class="crefs">
            <For each={refs()}>
              {(ref) => (
                <span class="cref" title={ref.path ?? ref.name}>
                  <RefIcon item={ref} />
                  <span>{ref.name}</span>
                  <button title="Remove" onClick={() => setRefs((all) => all.filter((r) => !sameRef(r, ref)))}>
                    <Icon name="x" small />
                  </button>
                </span>
              )}
            </For>
          </div>
        </Show>
        <div class="cb-top">
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
            onInput={onInput}
            onPaste={(e) => {
              const list = [...(e.clipboardData?.files ?? [])];
              if (!list.length) return;
              e.preventDefault();
              void attach(list);
            }}
            onKeyDown={(e) => {
              if (menuKey(e)) return;
              // Backspace at the start takes the last chip away.
              if (e.key === "Backspace" && input.selectionStart === 0 && input.selectionEnd === 0 && refs().length) {
                e.preventDefault();
                setRefs((all) => all.slice(0, -1));
                return;
              }
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
          <button
            class="cbi"
            classList={{ on: props.web }}
            aria-pressed={props.web}
            title={props.web ? "Web search: on" : "Web search: off. The agent works without the web"}
            onClick={() => props.onWeb(!props.web)}
          >
            <Icon name="globe" />
          </button>
          <button
            class="cbi"
            classList={{ on: props.context }}
            aria-pressed={props.context}
            title={
              props.context
                ? "Session context: on. Where you are in the editor, and what you selected, goes with every message"
                : "Session context: off. Where you are in the editor stays out of your messages"
            }
            onClick={() => props.onContext(!props.context)}
          >
            <Icon name={props.context ? "eye" : "eyeOff"} />
          </button>
          <button class="cbi mode" classList={{ named: props.mode.id !== "normal" }} title={`Mode: ${props.mode.label}. ${props.mode.detail}`} onClick={() => toggle("mode")}>
            <Icon name={props.mode.icon} />
            <Show when={props.mode.id !== "normal"}>
              <span>{props.mode.label}</span>
            </Show>
          </button>
          <span class="spacer" />
          <Show when={used()}>
            <button class="cchip tok" title={`Context: ${Math.round(props.status?.context ?? 0)}% used. Click for details`} onClick={() => toggle("context")}>
              <ContextRing percent={props.status?.context ?? 0} />
              <span>{used()}</span>
            </button>
          </Show>
          <button
            class="cchip model"
            title="Model and thinking effort"
            onClick={() => {
              setTab(undefined);
              toggle("model");
            }}
          >
            <ProviderIcon kind={props.kind.kind} />
            <span>
              {shortModel(props.model)}
              {/* JSX, not a string, so it changes as the effort does. */}
              <Show when={props.effort ?? props.kind.recommended}>{(effort) => <> ({effortName(effort())})</>}</Show>
            </span>
            <Icon name="chevron" small />
          </button>
        </div>
      </div>
      <input
        ref={files}
        type="file"
        multiple
        hidden
        onChange={() => {
          const list = [...(files.files ?? [])];
          files.value = "";
          void attach(list);
        }}
      />
      <Show when={popover() === "mention"}>
        <div class="apop mention" role="menu" aria-label="Add to the message">
          <Show when={mentions()} fallback={<div class="mm-h">Loading…</div>}>
            <Show when={groups().length} fallback={<div class="mm-h">{query() ? `Nothing matches “${query()}”` : "Nothing to add yet"}</div>}>
              <For each={groups()}>
                {([label, list]) => (
                  <>
                    <div class="mm-h">{label}</div>
                    <For each={list}>
                      {(mention) => (
                        <button
                          class="mm-i"
                          role="menuitem"
                          classList={{ on: shown()[active()] === mention }}
                          title={mention.ref.path ?? mention.ref.name}
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => void pick(mention)}
                        >
                          <RefIcon item={mention.ref} />
                          <span>{mention.ref.name}</span>
                          <kbd />
                        </button>
                      )}
                    </For>
                  </>
                )}
              </For>
            </Show>
          </Show>
          <div class="mm-sep" />
          <button
            class="mm-i"
            role="menuitem"
            onClick={() => {
              closeMention();
              files.click();
            }}
          >
            <Icon name="upload" />
            <span>Attach from this Mac…</span>
            <kbd>files, images, PDFs</kbd>
          </button>
        </div>
      </Show>
      <Show when={popover() === "mode"}>
        <div class="apop mode">
          <div class="ap-h">Mode</div>
          <For each={MODES.filter((m) => (m.offered && hasMode(props.kind, m)) || m.id === props.mode.id)}>
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
                    <small>{note() ?? props.kind.modeDetails?.[mode.id] ?? mode.detail}</small>
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
          <Show when={props.onCodeTo}>
            <div class="ap-h dest-h">Agent code goes</div>
            <div class="seg2" role="group" aria-label="Agent code goes">
              <For each={CODE_TO}>
                {([to, label, detail]) => (
                  <button
                    aria-pressed={(props.codeTo ?? "file") === to}
                    title={detail}
                    onClick={() => {
                      if ((props.codeTo ?? "file") !== to) props.onCodeTo?.(to);
                    }}
                  >
                    {label}
                  </button>
                )}
              </For>
            </div>
          </Show>
        </div>
      </Show>
      <Show when={popover() === "model"}>
        <div class="apop model" role="dialog" aria-label="Model and thinking effort">
          <div class="mt-tabs" role="tablist">
            <button class="mt-t" classList={{ on: openTab() === "fav" }} role="tab" aria-selected={openTab() === "fav"} title="Favourites" onClick={() => setTab("fav")}>
              <Icon name="star" />
            </button>
            <For each={props.kinds}>
              {(kind) => (
                <button
                  class="mt-t"
                  classList={{ on: openTab() === kind.kind }}
                  role="tab"
                  aria-selected={openTab() === kind.kind}
                  disabled={!usable(kind)}
                  title={`${kind.name} · ${signIn(kind)}${usable(kind) ? "" : " · start a new conversation to use it"}`}
                  onClick={() => setTab(kind.kind)}
                >
                  <ProviderIcon kind={kind.kind} />
                </button>
              )}
            </For>
          </div>
          <div class="mt-sub">
            {openTab() === "fav" ? "Favourites from every agent" : (kind => `${kind.name} · ${signIn(kind)}`)(props.kinds.find((k) => k.kind === openTab()) ?? props.kind)}
          </div>
          <Show when={props.modelNote}>
            <p class="ap-n">{props.modelNote}</p>
          </Show>
          <div class="mt-list" role="listbox" aria-label="Models">
            <For each={rows()} fallback={<div class="mt-empty">Star a model to keep it here.</div>}>
              {(row, i) => (
                <div
                  class="mt-row"
                  classList={{ on: props.isModel(row.kind.kind, row.model), off: !usable(row.kind) }}
                  role="option"
                  tabIndex={0}
                  aria-selected={props.isModel(row.kind.kind, row.model)}
                  title={`${row.kind.name}: ${row.model.label}${usable(row.kind) ? "" : " · start a new conversation to use it"}`}
                  onClick={() => choose(row)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      choose(row);
                    }
                  }}
                >
                  <Show when={openTab() === "fav"}>
                    <ProviderIcon kind={row.kind.kind} />
                  </Show>
                  <span class="mt-n">{row.model.label}</span>
                  <Show when={i() < 9}>
                    <kbd>⌥{i() + 1}</kbd>
                  </Show>
                  <button
                    class="mt-star"
                    classList={{ on: stars().includes(modelKey(row.kind.kind, row.model)) }}
                    title={stars().includes(modelKey(row.kind.kind, row.model)) ? "Remove from favourites" : "Add to favourites"}
                    onClick={(e) => {
                      e.stopPropagation();
                      star(row.kind, row.model);
                    }}
                  >
                    <Icon name={stars().includes(modelKey(row.kind.kind, row.model)) ? "starFilled" : "star"} />
                  </button>
                </div>
              )}
            </For>
          </div>
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

/** "your Claude plan": how the agent signs in, from its plan line. */
function signIn(kind: AgentKind): string {
  return kind.plan.split(" · ").pop() ?? kind.plan;
}

function RefIcon(props: { item: MessageRef }) {
  const kind = () => props.item.kind;
  return (
    <Show when={kind() === "file" || kind() === "attach"} fallback={<Icon name={kind() === "chunk" ? "outline" : kind() === "plot" ? "zoom" : "table"} />}>
      <FileIcon name={baseName(props.item.path ?? props.item.name)} />
    </Show>
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
