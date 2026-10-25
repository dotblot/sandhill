// An agent's conversation as a chat (draft 18, after Posit Assistant): your
// messages in grey bubbles, the agent's replies as plain text, a row per step
// that opens to show its code, output and plots, and cards for what waits on
// you: a permission, a question, or a plan.

import DOMPurify from "dompurify";
import { marked } from "marked";
import { createEffect, createMemo, createSignal, For, type JSX, Match, on, onCleanup, Show, Switch } from "solid-js";
import { highlightedLines, languageFor } from "../editor/setup";
import { type Answer, type Ask, type AskView, askView, type Chat, type ChatItem, type Place, type Question, type Step, type StepView, stepView } from "../model/chat";
import { appendAnsi } from "../render/ansi";
import { Icon } from "./icons";

/** Within this many pixels of the bottom, the chat follows new messages. */
const FOLLOW_PX = 48;
const COPIED_MS = 1400;

export function AgentChat(props: {
  chat: Chat;
  /** The agent's name: "Claude". */
  agent: string;
  /** Answers sent and not yet recorded, by ask id. */
  sent: Set<string>;
  onAnswer: (ask: Ask, answer: Answer) => void;
  /** Opens a file the agent names, relative to the project or absolute. */
  onOpen: (path: string) => void;
  onLink: (url: string) => void;
  /** Adds code as a cell below the active cell of the linked file. */
  onInsert?: (code: string, language?: string) => void;
  /** The file the conversation is linked to, where cells go by default. */
  linked?: string;
  /** Bumped when the user sends a message: scroll to the end. */
  sentAt: number;
  children?: JSX.Element;
}) {
  const [open, setOpen] = createSignal<Set<string>>(new Set());
  const toggle = (key: string) =>
    setOpen((keys) => {
      const next = new Set(keys);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  // Items keep their place by key, so a step that finishes updates in place.
  const byKey = createMemo(() => new Map(props.chat.items.map((item) => [item.key, item])));
  const keys = createMemo(() => props.chat.items.map((item) => item.key), undefined, {
    equals: (a, b) => a.length === b.length && a.every((k, i) => k === b[i]),
  });
  const waiting = createMemo(() => new Set(props.chat.waiting.map((a) => a.id)));

  let scroller!: HTMLDivElement;
  let following = true;
  const toEnd = () => requestAnimationFrame(() => (scroller.scrollTop = scroller.scrollHeight));
  createEffect(
    on(
      () => props.chat,
      () => {
        if (following) toEnd();
      },
    ),
  );
  createEffect(
    on(
      () => props.sentAt,
      () => {
        following = true;
        toEnd();
      },
      { defer: true },
    ),
  );

  return (
    <div
      class="chat"
      ref={scroller}
      onScroll={() => (following = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < FOLLOW_PX)}
    >
      <For each={keys()}>
        {(key) => {
          const item = () => byKey().get(key);
          return (
            <Show when={item()}>
              {(it) => (
                <ItemView
                  item={it()}
                  agent={props.agent}
                  open={open().has(key)}
                  waiting={waiting()}
                  sent={props.sent}
                  onToggle={() => toggle(key)}
                  onAnswer={props.onAnswer}
                  onOpen={props.onOpen}
                  onLink={props.onLink}
                  onInsert={props.onInsert}
                  linked={props.linked}
                />
              )}
            </Show>
          );
        }}
      </For>
      {props.children}
    </div>
  );
}

function ItemView(props: {
  item: ChatItem;
  agent: string;
  open: boolean;
  waiting: Set<string>;
  sent: Set<string>;
  onToggle: () => void;
  onAnswer: (ask: Ask, answer: Answer) => void;
  onOpen: (path: string) => void;
  onLink: (url: string) => void;
  onInsert?: (code: string, language?: string) => void;
  linked?: string;
}) {
  const ask = (a: Ask | undefined) => (a && props.waiting.has(a.id) ? a : undefined);
  return (
    <Switch>
      <Match when={props.item.kind === "user" && props.item}>
        {(item) => <UserMessage text={item().text} place={item().place} onOpen={props.onOpen} />}
      </Match>
      <Match when={props.item.kind === "reply" && props.item}>
        {(item) => <Reply text={item().text} onOpen={props.onOpen} onLink={props.onLink} />}
      </Match>
      <Match when={props.item.kind === "step" && ask((props.item as Step).ask)}>
        {(a) => <AskCard ask={a()} agent={props.agent} linked={props.linked} sent={props.sent.has(a().id)} onAnswer={props.onAnswer} onOpen={props.onOpen} />}
      </Match>
      <Match when={props.item.kind === "step" && (props.item as Step)}>
        {(step) => <StepRow step={step()} linked={props.linked} open={props.open} onToggle={props.onToggle} onOpen={props.onOpen} onInsert={props.onInsert} />}
      </Match>
      <Match when={props.item.kind === "ask" && props.item}>
        {(item) => (
          <Show
            when={ask(item().ask)}
            fallback={<AnsweredRow ask={item().ask} linked={props.linked} open={props.open} onToggle={props.onToggle} onOpen={props.onOpen} />}
          >
            {(a) => <AskCard ask={a()} agent={props.agent} linked={props.linked} sent={props.sent.has(a().id)} onAnswer={props.onAnswer} onOpen={props.onOpen} />}
          </Show>
        )}
      </Match>
      <Match when={props.item.kind === "footer" && props.item}>{(item) => <TurnFooter at={item().at_ms} ms={item().ms} text={item().text} />}</Match>
      <Match when={props.item.kind === "note" && props.item}>{(item) => <div class="chat-note">{item().text}</div>}</Match>
      <Match when={props.item.kind === "error" && props.item}>
        {(item) => (
          <div class="chat-error" role="alert">
            <Icon name="failed" />
            <span>{item().text}</span>
          </div>
        )}
      </Match>
      <Match when={props.item.kind === "thinking"}>
        <div class="thinking">
          <span class="spin" />
          Thinking…
        </div>
      </Match>
    </Switch>
  );
}

function UserMessage(props: { text: string; place?: Place; onOpen: (path: string) => void }) {
  return (
    <div class="umsg">
      <div class="ubub">{props.text}</div>
      <Show when={props.place}>
        {(place) => (
          <div class="uctx" title="Sent with this message">
            <Icon name="eye" />
            <Show when={place().file}>
              {(file) => (
                <>
                  <button class="flink" onClick={() => props.onOpen(file())}>
                    {baseName(file())}
                  </button>
                  <Show when={place().cell ?? (place().line ? `line ${place().line}` : undefined)}>{(where) => <span>› {where()}</span>}</Show>
                </>
              )}
            </Show>
            <For each={place().refs ?? []}>{(name) => <span class="uref">@{name}</span>}</For>
          </div>
        )}
      </Show>
    </div>
  );
}

/** A reply in Markdown. Code spans become grey pills, and file names in them open the file. */
function Reply(props: { text: string; onOpen: (path: string) => void; onLink: (url: string) => void }) {
  let el!: HTMLDivElement;
  createEffect(() => {
    el.innerHTML = markdown(props.text);
    for (const code of el.querySelectorAll<HTMLElement>(":not(pre) > code")) {
      code.classList.add("ipill");
      const file = FILE_NAME.exec(code.textContent ?? "")?.[1];
      if (file) {
        code.classList.add("flink");
        code.dataset.file = file;
        code.title = `Open ${file}`;
      }
    }
    for (const code of el.querySelectorAll<HTMLElement>("pre > code")) {
      const language = languageFor(/language-(\w+)/.exec(code.className)?.[1]);
      if (language) code.replaceChildren(...highlightedLines(code.textContent?.replace(/\n$/, "") ?? "", language));
    }
  });
  const click = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    const file = target.closest<HTMLElement>("[data-file]")?.dataset.file;
    if (file) {
      props.onOpen(file);
      return;
    }
    const link = target.closest("a");
    if (link?.href) {
      e.preventDefault();
      props.onLink(link.href);
    }
  };
  return <div class="amsg" ref={el} onClick={click} />;
}

/** A file name or path, maybe with a line: `analysis.qmd`, `R/qc.R:26`. */
const FILE_NAME = /^([\w./~-]+\.(?:qmd|rmd|ipynb|r|py|md|txt|csv|tsv|json|ya?ml|toml|sh|rds|h5ad))(?::\d+(?::\d+)?)?$/i;

function markdown(text: string): string {
  return DOMPurify.sanitize(marked.parse(text, { async: false, gfm: true }) as string);
}

const STATUS_TITLES: Record<Step["status"], string> = {
  run: "Running",
  wait: "Waiting for you",
  done: "Done",
  fail: "Failed",
  deny: "You denied this step",
  cancel: "Stopped",
};

function StepRow(props: {
  step: Step;
  linked?: string;
  open: boolean;
  onToggle: () => void;
  onOpen: (path: string) => void;
  onInsert?: (code: string, language?: string) => void;
}) {
  const view = createMemo(() => stepView(props.step, props.linked));
  return (
    <div class={`step st-${props.step.status}`} classList={{ open: props.open }}>
      <div
        class="st-row"
        role="button"
        tabindex="0"
        aria-expanded={props.open}
        onClick={(e) => {
          if (!(e.target as Element).closest(".flink")) props.onToggle();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            props.onToggle();
          }
        }}
      >
        <span class="st-ic" title={STATUS_TITLES[props.step.status]}>
          <Switch fallback={<Icon name="failed" />}>
            <Match when={props.step.status === "run"}>
              <span class="spin st-spin" />
            </Match>
            <Match when={props.step.status === "wait"}>
              <Icon name="hand" />
            </Match>
            <Match when={props.step.status === "done"}>
              <Icon name="done" />
            </Match>
          </Switch>
        </span>
        <span class="st-main">
          <b class="st-tool">{view().name}</b>
          <Show when={view().desc}>
            {" "}
            <Description text={view().desc} file={view().file} onOpen={props.onOpen} />
          </Show>
        </span>
        <span class="st-det">
          <Show when={view().added !== undefined} fallback={view().detail}>
            <span class="d-add">+{view().added}</span> <span class="d-del">−{view().removed}</span>
          </Show>
        </span>
        <span class="st-chev">
          <Icon name="chevronRight" />
        </span>
      </div>
      <Show when={props.open}>
        <div class="st-body">
          <StepBody view={view()} error={props.step.error} denied={props.step.status === "deny"} onInsert={props.onInsert} />
        </div>
      </Show>
    </div>
  );
}

/** A step's description, with the file it names as a link. */
function Description(props: { text: string; file?: string; onOpen: (path: string) => void }) {
  const parts = () => {
    const name = props.file ? baseName(props.file) : "";
    const at = name ? props.text.indexOf(name) : -1;
    return at < 0 ? undefined : [props.text.slice(0, at), name, props.text.slice(at + name.length)];
  };
  return (
    <span class="st-desc">
      <Show when={parts()} fallback={props.text}>
        {(p) => (
          <>
            {p()[0]}
            {/* A span, not a button, so a long name is cut with an ellipsis rather than dropped whole. */}
            <span
              class="flink"
              role="link"
              tabindex="0"
              title={`Open ${props.file}`}
              onClick={(e) => {
                e.stopPropagation();
                props.onOpen(props.file!);
              }}
              onKeyDown={(e) => {
                if (e.key !== "Enter") return;
                e.stopPropagation();
                props.onOpen(props.file!);
              }}
            >
              {p()[1]}
            </span>
            {p()[2]}
          </>
        )}
      </Show>
    </span>
  );
}

function StepBody(props: { view: StepView; error?: string; denied?: boolean; onInsert?: (code: string, language?: string) => void }) {
  const v = () => props.view;
  const runnable = () => !!v().code && (v().language === "r" || v().language === "python") && v().name === "run_code";
  return (
    <Switch
      fallback={
        <Show when={v().output || props.error} fallback={<p class="st-note">{props.denied ? "You denied this step." : "No output."}</p>}>
          <Output text={props.error ?? v().output ?? ""} error={!!props.error} />
          <Images images={v().images} />
        </Show>
      }
    >
      <Match when={v().diff}>
        {(diff) => (
          <div class="cblock">
            <Diff lines={diff()} />
            <Show when={v().output || props.error}>
              <Output text={props.error ?? v().output ?? ""} error={!!props.error} />
            </Show>
            <Images images={v().images} />
          </div>
        )}
      </Match>
      <Match when={v().code}>
        {(code) => (
          <div class="cblock">
            <div class="cb-h">
              <span>{languageLabel(v().language)}</span>
              <span class="spacer" />
              <span>{v().detail}</span>
            </div>
            <Code code={code()} language={v().language} />
            <Show when={v().output || props.error}>
              <Output text={props.error ?? v().output ?? ""} error={!!props.error} />
            </Show>
            <Images images={v().images} />
            <div class="cb-acts">
              <CopyButton text={code()} />
              <Show when={runnable() && props.onInsert}>
                <button title="Add as a cell below your active cell in the linked file" onClick={() => props.onInsert!(code(), v().language)}>
                  <Icon name="insert" />
                  Insert into file
                </button>
              </Show>
            </div>
          </div>
        )}
      </Match>
    </Switch>
  );
}

function Code(props: { code: string; language?: string }) {
  let el!: HTMLPreElement;
  createEffect(() => el.replaceChildren(...highlightedLines(props.code, languageFor(props.language))));
  return <pre class="cb-code" ref={el} />;
}

function Output(props: { text: string; error?: boolean }) {
  let el!: HTMLPreElement;
  createEffect(() => {
    el.replaceChildren();
    appendAnsi(el, props.text);
  });
  return <pre class="cb-out" classList={{ err: props.error }} ref={el} />;
}

function Diff(props: { lines: string[] }) {
  return (
    <pre class="cb-code diff">
      <For each={props.lines}>
        {(line) => (
          <span class="dl" classList={{ add: line.startsWith("+"), del: line.startsWith("-") }}>
            {line.charAt(0) === " " ? " " : line.charAt(0)} {line.slice(1)}
          </span>
        )}
      </For>
    </pre>
  );
}

function Images(props: { images: string[] }) {
  return (
    <For each={props.images}>
      {(src) => (
        <div class="cb-plot">
          <img src={src} alt="Plot" />
        </div>
      )}
    </For>
  );
}

function CopyButton(props: { text: string; label?: boolean; class?: string; title?: string }) {
  const [copied, setCopied] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));
  const copy = () => {
    void navigator.clipboard?.writeText(props.text).then(() => {
      setCopied(true);
      clearTimeout(timer);
      timer = setTimeout(() => setCopied(false), COPIED_MS);
    });
  };
  return (
    <button class={props.class} title={props.title ?? "Copy"} onClick={copy}>
      <Icon name={copied() ? "check" : "copy"} />
      <Show when={props.label !== false}>{copied() ? "Copied" : "Copy"}</Show>
    </button>
  );
}

/** A card for what waits on the user: a permission, a question, or a plan. */
function AskCard(props: { ask: Ask; agent: string; linked?: string; sent: boolean; onAnswer: (ask: Ask, answer: Answer) => void; onOpen: (path: string) => void }) {
  const view = createMemo(() => askView(props.ask, props.linked, props.agent));
  const answer = (a: Answer) => {
    if (!props.sent) props.onAnswer(props.ask, a);
  };
  return (
    <Switch fallback={<Permission ask={props.ask} view={view()} sent={props.sent} onAnswer={answer} onOpen={props.onOpen} />}>
      <Match when={view().questions}>
        {(questions) => <Questions agent={props.agent} questions={questions()} sent={props.sent} onAnswer={answer} />}
      </Match>
      <Match when={view().plan !== undefined}>
        <PlanCard plan={view().plan ?? ""} remember={props.ask.remember} sent={props.sent} onAnswer={answer} />
      </Match>
    </Switch>
  );
}

function Permission(props: { ask: Ask; view: AskView; sent: boolean; onAnswer: (a: Answer) => void; onOpen: (path: string) => void }) {
  const v = () => props.view.view;
  const remember = () => props.ask.remember ?? [];
  const also = () => `Also allows ${remember().join(", ")}`;
  /** For how long it can be allowed: both, unless the agent says. */
  const scope = (s: string) => remember().length > 0 && (props.ask.scopes ?? ["chat", "project"]).includes(s);
  return (
    <div class="approve" role="group" aria-label="Approval needed">
      <div class="ap-top">
        <Icon name="hand" />
        <span>
          <b>
            <Description text={props.view.title} file={props.view.file} onOpen={props.onOpen} />
          </b>
          <Show when={props.view.detail}>
            <small>{props.view.detail}</small>
          </Show>
        </span>
        <Show when={v().added !== undefined}>
          <span class="d-add">+{v().added}</span>
        </Show>
      </div>
      <Switch fallback={<Show when={v().output}>{(out) => <pre class="cb-code">{out()}</pre>}</Show>}>
        <Match when={v().diff}>{(diff) => <Diff lines={diff()} />}</Match>
        <Match when={v().code}>{(code) => <Code code={code()} language={v().language} />}</Match>
      </Switch>
      <div class="ap-btns">
        <button class="apb pri" disabled={props.sent} onClick={() => props.onAnswer({ behavior: "allow", scope: "once" })}>
          Allow once
        </button>
        <Show when={scope("chat")}>
          <button class="apb" disabled={props.sent} title={`${also()} for the rest of this conversation`} onClick={() => props.onAnswer({ behavior: "allow", scope: "chat" })}>
            For this chat
          </button>
        </Show>
        <Show when={scope("project")}>
          <button class="apb" disabled={props.sent} title={`${also()} in this project from now on`} onClick={() => props.onAnswer({ behavior: "allow", scope: "project" })}>
            For this project
          </button>
        </Show>
        <span class="spacer" />
        <button class="apb deny" disabled={props.sent} onClick={() => props.onAnswer({ behavior: "deny" })}>
          Deny
        </button>
      </div>
    </div>
  );
}

/** Claude's questions, each with its options; "Other" takes your own words. */
function Questions(props: { agent: string; questions: Question[]; sent: boolean; onAnswer: (a: Answer) => void }) {
  const [picked, setPicked] = createSignal<Record<string, string[]>>({});
  const [other, setOther] = createSignal<Record<string, string>>({});
  const choose = (q: Question, label: string) =>
    setPicked((all) => {
      const now = all[q.question] ?? [];
      const next = q.multiSelect ? (now.includes(label) ? now.filter((l) => l !== label) : [...now, label]) : [label];
      return { ...all, [q.question]: next };
    });
  const answerOf = (q: Question) => [...(picked()[q.question] ?? []), ...(other()[q.question]?.trim() ? [other()[q.question].trim()] : [])].join(", ");
  const ready = () => props.questions.every((q) => answerOf(q));
  const send = () => props.onAnswer({ behavior: "allow", answers: Object.fromEntries(props.questions.map((q) => [q.question, answerOf(q)])) });
  return (
    <div class="approve question" role="group" aria-label="Question">
      <For each={props.questions}>
        {(q) => (
          <div class="q">
            <div class="ap-top">
              <Icon name="help" />
              <span>
                <b>{q.question}</b>
                <Show when={q.multiSelect}>
                  <small>Pick any that apply</small>
                </Show>
              </span>
              <Show when={q.header}>
                <span class="q-h">{q.header}</span>
              </Show>
            </div>
            <div class="q-opts">
              <For each={q.options}>
                {(option) => (
                  <button
                    class="q-opt"
                    classList={{ on: (picked()[q.question] ?? []).includes(option.label) }}
                    aria-pressed={(picked()[q.question] ?? []).includes(option.label)}
                    disabled={props.sent}
                    onClick={() => choose(q, option.label)}
                  >
                    <b>{option.label}</b>
                    <Show when={option.description}>
                      <small>{option.description}</small>
                    </Show>
                  </button>
                )}
              </For>
              <input
                class="q-other"
                placeholder="Other…"
                disabled={props.sent}
                value={other()[q.question] ?? ""}
                onInput={(e) => {
                  const text = e.currentTarget.value;
                  setOther((all) => ({ ...all, [q.question]: text }));
                  if (!q.multiSelect && text.trim()) setPicked((all) => ({ ...all, [q.question]: [] }));
                }}
              />
            </div>
          </div>
        )}
      </For>
      <div class="ap-btns">
        <button class="apb pri" disabled={!ready() || props.sent} onClick={send}>
          Answer
        </button>
        <span class="spacer" />
        <button
          class="apb"
          disabled={props.sent}
          onClick={() => props.onAnswer({ behavior: "deny", message: `The user skipped your questions in Sandhill. Ask them in the chat instead.` })}
        >
          Skip
        </button>
      </div>
    </div>
  );
}

function PlanCard(props: { plan: string; remember?: string[]; sent: boolean; onAnswer: (a: Answer) => void }) {
  let el!: HTMLDivElement;
  createEffect(() => (el.innerHTML = markdown(props.plan)));
  return (
    <div class="approve plan" role="group" aria-label="Plan to approve">
      <div class="ap-top">
        <Icon name="plan" />
        <span>
          <b>Carry out this plan?</b>
          <small>Nothing has changed yet.</small>
        </span>
      </div>
      <div class="plan-md amsg" ref={el} />
      <div class="ap-btns">
        <button class="apb pri" disabled={props.sent} onClick={() => props.onAnswer({ behavior: "allow", scope: "once" })}>
          Yes, carry it out
        </button>
        <Show when={props.remember?.length}>
          <button
            class="apb"
            disabled={props.sent}
            title="Carry it out, and let it edit files without asking"
            onClick={() => props.onAnswer({ behavior: "allow", scope: "chat" })}
          >
            Yes, and allow edits
          </button>
        </Show>
        <span class="spacer" />
        <button
          class="apb"
          disabled={props.sent}
          onClick={() => props.onAnswer({ behavior: "deny", message: "The user wants to keep planning. Ask them what to change in the plan." })}
        >
          Keep planning
        </button>
      </div>
    </div>
  );
}

/** A request that was answered, or given up, with no step to show it on. */
function AnsweredRow(props: { ask: Ask; linked?: string; open: boolean; onToggle: () => void; onOpen: (path: string) => void }) {
  const view = createMemo(() => askView(props.ask, props.linked));
  const how = () => {
    const answer = props.ask.answer;
    if (!answer) return "Not answered here";
    if (answer.answers) return Object.values(answer.answers).join("; ");
    return answer.behavior === "allow" ? "Allowed" : "Denied";
  };
  const allowed = () => props.ask.answer?.behavior === "allow";
  return (
    <div class="step" classList={{ open: props.open, "st-deny": !allowed() }}>
      <div class="st-row" role="button" tabindex="0" aria-expanded={props.open} onClick={props.onToggle}>
        <span class="st-ic">
          <Icon name={allowed() ? "done" : "failed"} />
        </span>
        <span class="st-main">
          <Description text={view().questions ? (view().questions?.[0]?.question ?? view().title) : view().title} file={view().file} onOpen={props.onOpen} />
        </span>
        <span class="st-det">{how()}</span>
        <span class="st-chev">
          <Icon name="chevronRight" />
        </span>
      </div>
      <Show when={props.open && view().plan}>
        {(plan) => (
          <div class="st-body">
            <Reply text={plan()} onOpen={props.onOpen} onLink={() => {}} />
          </div>
        )}
      </Show>
    </div>
  );
}

function TurnFooter(props: { at?: number; ms?: number; text: string }) {
  return (
    <div class="tfoot">
      <Show when={props.text}>
        <CopyButton class="tf-copy" text={props.text} label={false} title="Copy this reply" />
      </Show>
      <Show when={props.at}>{(at) => <span>{when(at())}</span>}</Show>
      <Show when={props.ms !== undefined && props.ms > 0}>
        <span>{duration(props.ms!)}</span>
      </Show>
    </div>
  );
}

/** "Today 14:31", "Yesterday 16:20", "Oct 8 11:02". */
export function when(ms: number, now = Date.now()): string {
  const date = new Date(ms);
  const time = date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((day(new Date(now)) - day(date)) / 86_400_000);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Yesterday ${time}`;
  return `${date.toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${time}`;
}

/** "12 s", "3 min 5 s". */
function duration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${Math.max(1, s)} s`;
  const m = Math.floor(s / 60);
  return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
}

function languageLabel(language?: string): string {
  if (!language) return "Code";
  if (language === "r") return "R";
  if (language === "shell") return "Shell";
  return language.charAt(0).toUpperCase() + language.slice(1);
}

function baseName(path: string): string {
  return path.split("/").pop() ?? path;
}
