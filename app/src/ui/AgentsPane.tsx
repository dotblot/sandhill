import { createEffect, createMemo, createResource, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, untrack } from "solid-js";
import { LOCAL_HOST } from "../backend";
import type { AgentInfo, AgentModel, AgentOptions, AgentReport, FileStat, MessageRef, SessionPlots, TerminalInfo, Variables } from "../backend/types";
import type { Doc } from "../docs/doc";
import {
  AGENT_KINDS,
  type AgentKind,
  agentKind,
  canResume,
  type CodeTo,
  codeTo,
  currentMode,
  documentPath,
  drivesChat,
  effortLevel,
  effortName,
  listedKind,
  liveOptions,
  type Mode,
  MODES,
  type ModelChoice,
  modeFor,
  modelLabel,
  SUBMIT_DELAY_MS,
  screenMode,
  shortModel,
  withModels,
} from "../model/agents";
import { type Answer, type Ask, buildChat, chatTitle, lastActive } from "../model/chat";
import { EXPORT_FORMATS, type ExportFormat, exportConversation, exportName } from "../model/export";
import { ATTACHMENTS, attachmentName, type Mention } from "../model/mentions";
import { fromJournal, type PlotItem } from "../model/plots";
import type { TerminalStore } from "../model/terminals";
import type { Workspace } from "../model/workspace";
import { AgentChat, when } from "./AgentChat";
import { AgentComposer } from "./AgentComposer";
import { ask, askText, notify } from "./dialogs";
import { FileIcon, Icon, ProviderIcon } from "./icons";
import { type Pane, PaneButtons } from "./PaneButtons";
import { imageData, toBase64 } from "./plotImage";
import { TermView } from "./termView";

/** The @ menu offers this many of the project's latest plots. */
const PLOTS_OFFERED = 12;
/** Effort is sent to the agent once the knob has rested this long. */
const EFFORT_SETTLE_MS = 600;
/** A conversation that shows nothing this long after it started may be asking something in its terminal. */
const START_GRACE_MS = 8000;
/** Switching mode presses ⇧Tab at most this often, this far apart, until the terminal shows the mode. */
const MODE_TRIES = 6;
const MODE_STEP_MS = 350;
/** Between moving to "Yes, I trust this folder" and choosing it. */
const TRUST_KEY_MS = 500;

/**
 * The Agents pane (draft 18): one conversation at a time as a chat, with a
 * history of the project's conversations and a new-conversation view. Claude
 * Code, Codex and Antigravity run unmodified in terminals the daemon keeps,
 * and the terminal is a click away. Claude Code's chat is built from what
 * its hooks report, and it takes messages as typed keys. Codex runs as its
 * app-server, and Antigravity's agy headless for each message, under a
 * bridge in the daemon, which writes the chat and takes messages, models
 * and modes as commands. Sessions started before chats existed show their
 * terminal.
 */
export function AgentsPane(props: {
  ws: Workspace;
  store: TerminalStore;
  project: string;
  hostLabel: string;
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
  onOpen: (path: string) => Promise<Doc | undefined>;
  /** A message for the agent from an error's Fix or Explain button; `onHandoff` says it was taken. */
  handoff?: { text: string; refs: MessageRef[] };
  onHandoff?: () => void;
}) {
  const store = props.store;
  const [picking, setPicking] = createSignal(false);
  const [history, setHistory] = createSignal(false);
  const [gear, setGear] = createSignal(false);
  /** The view of a session being started, before the daemon has it. */
  const [starting, setStarting] = createSignal<{ view: TermView; kind: string }>();
  /** Outdated sessions the user chose to keep using without the notice. */
  const [keptOld, setKeptOld] = createSignal<string[]>([]);
  /** Conversations showing their terminal instead of the chat. */
  const [termShown, setTermShown] = createSignal<string[]>([]);
  /** Asks answered here, until the daemon records the answer. */
  const [sent, setSent] = createSignal<Set<string>>(new Set());
  /** Conversations stopped here: Claude Code reports no end to a turn it was stopped in. */
  const [stopped, setStopped] = createSignal<string[]>([]);
  /** Conversations whose agent is being started again. */
  const [restarting, setRestarting] = createSignal<string[]>([]);
  /** Modes switched to here, until the agent reports one. */
  const [modeChoice, setModeChoice] = createSignal<Record<string, Mode>>({});
  const [effortChoice, setEffortChoice] = createSignal<Record<string, string>>({});
  /** Models picked here, until the agent reports one: Claude Code reports it with its next status line. */
  const [modelChoice, setModelChoice] = createSignal<Record<string, ModelChoice>>({});
  /** Where code goes, picked here for an ended conversation: it applies as it starts again. */
  const [codeChoice, setCodeChoice] = createSignal<Record<string, CodeTo>>({});
  const [sentAt, setSentAt] = createSignal(0);
  const [now, setNow] = createSignal(Date.now());
  /** The current terminal's last lines, while its conversation hasn't started. */
  const [screen, setScreen] = createSignal("");
  /** The conversation whose title is being renamed in the header. */
  const [renaming, setRenaming] = createSignal<string>();
  /** The message box's choices (pin, web, context) picked here for an ended conversation: they apply as it starts again. */
  const [boxChoice, setBoxChoice] = createSignal<Record<string, Pick<AgentOptions, "pin" | "web" | "context">>>({});
  /** A message put in a conversation's box (or "new", the new conversation's) to send from there. */
  const [fill, setFill] = createSignal<{ key: string; text: string; refs: MessageRef[] }>();
  const views = new Map<string, TermView>();
  let body!: HTMLDivElement;
  let input: HTMLTextAreaElement | undefined;

  const current = (): TerminalInfo | undefined => {
    const agents = store.agents();
    return agents.find((t) => t.id === props.selected) ?? agents[agents.length - 1];
  };
  const relative = (path: string) => (path.startsWith(`${props.project}/`) ? path.slice(props.project.length + 1) : path);
  /** Conversations show as a chat; those started before chats existed keep their terminal. */
  const canChat = (t: TerminalInfo) =>
    (t.kind === "claude" || drivesChat(t.kind)) && (store.chat(t.id).length > 0 || (!t.outdated && t.running));
  /** Conversations whose chat Sandhill drives (Codex, Antigravity): what the user does goes to it as commands. */
  const driven = (t: TerminalInfo) => drivesChat(t.kind) && canChat(t);
  /** A new conversation that shows as a chat while it starts. */
  const chatStarting = () => {
    const kind = starting()?.kind;
    return kind !== undefined && (kind === "claude" || drivesChat(kind));
  };
  const chatMode = (t: TerminalInfo) => canChat(t) && !termShown().includes(t.id);
  const working = (t: TerminalInfo) =>
    t.running && (t.agent?.state === "busy" || t.agent?.state === "waiting") && !stopped().includes(t.id);
  const title = (t: TerminalInfo) => t.name || chatTitle(store.chat(t.id)) || `${t.title} · ${modelLabel(t)}`;
  /** The conversation's entries since the agent last started. */
  const sinceStart = (t: TerminalInfo) => {
    const started = t.started_ms ?? t.created_ms;
    return store.chat(t.id).filter((e) => (e.event.at_ms ?? started) >= started).length;
  };
  /** Ended, with a conversation the agent can go on with. */
  const resumable = (t: TerminalInfo) => !t.running && canResume(t) && chatMode(t) && !restarting().includes(t.id);
  const chat = createMemo(() => {
    const t = current();
    return t && canChat(t) ? buildChat(store.chat(t.id), working(t)) : undefined;
  });

  const viewFor = (id: string): TermView => {
    let view = views.get(id);
    if (!view) {
      view = new TermView(store, (url) => props.ws.backend.openLink(url));
      views.set(id, view);
    }
    return view;
  };

  // The current conversation's terminal is always mounted, behind its chat if
  // it has one: it keeps its size, and the mode can be read from it. The
  // others are kept alive but hidden.
  const shown = createMemo(() => (picking() || history() || starting() ? undefined : current()?.id));
  createEffect(
    on(shown, (id) => {
      for (const [other, view] of views) view.el.hidden = other !== id;
      if (!id) return;
      const view = viewFor(id);
      view.el.hidden = false;
      void view.mount(body).then(() => {
        view.bind(id);
        if (props.min) return;
        const t = current();
        if (t && chatMode(t)) input?.focus();
        else view.focus();
      });
    }),
  );
  // Sessions that were closed, here or elsewhere, take their views with them.
  createEffect(() => {
    const ids = new Set(store.list().map((t) => t.id));
    for (const [id, view] of views) {
      if (ids.has(id) || view === starting()?.view) continue;
      view.dispose();
      views.delete(id);
    }
  });
  onCleanup(() => {
    for (const view of views.values()) view.dispose();
  });
  // A mode or effort the agent reports replaces the one picked here.
  createEffect(
    on(
      () => [current()?.agent?.mode ?? current()?.agent?.permission, current()?.agent?.guard].join(),
      () => {
        const id = current()?.id;
        if (id) setModeChoice(({ [id]: _, ...rest }) => rest);
      },
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => current()?.agent?.status?.effort,
      () => {
        const id = current()?.id;
        if (id) setEffortChoice(({ [id]: _, ...rest }) => rest);
      },
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => current()?.agent?.status?.model_id ?? current()?.agent?.status?.model,
      () => {
        const id = current()?.id;
        if (id) setModelChoice(({ [id]: _, ...rest }) => rest);
      },
      { defer: true },
    ),
  );
  // While a new or resumed conversation shows nothing yet, check now and then whether it is stuck.
  createEffect(() => {
    const t = current();
    if (!t?.running || !chatMode(t) || sinceStart(t)) return;
    const timer = setInterval(() => {
      setNow(Date.now());
      setScreen(views.get(t.id)?.screenText(24) ?? "");
    }, 1000);
    onCleanup(() => clearInterval(timer));
  });
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || e.defaultPrevented) return;
    if (gear()) setGear(false);
    else if (history() && store.agents().length) setHistory(false);
    else if (picking() && store.agents().length) setPicking(false);
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  const closeGear = (e: MouseEvent) => {
    if (!(e.target as Element).closest(".apop.gear, .gear-btn")) setGear(false);
  };
  document.addEventListener("mousedown", closeGear);
  onCleanup(() => document.removeEventListener("mousedown", closeGear));

  // An error from Fix or Explain goes to the conversation in front, if its
  // agent is running; else into a box, to send from there.
  createEffect(
    on(
      () => props.handoff,
      (handoff) => {
        if (!handoff) return;
        props.onHandoff?.();
        const t = current();
        if (!t || !(t.running || resumable(t))) {
          openNew();
          setFill({ key: "new", ...handoff });
          return;
        }
        setPicking(false);
        setHistory(false);
        props.onSelect(t.id);
        if (!t.running || !send(t, handoff.text, handoff.refs)) setFill({ key: t.id, ...handoff });
      },
    ),
  );

  // Codex and Antigravity list their models for a new conversation's
  // menu: asked once as it first opens here, and taken from any
  // conversation that reports them.
  const askedModels = new Set<string>();
  createEffect(() => {
    if (!picking()) return;
    for (const kind of AGENT_KINDS.filter((k) => drivesChat(k.kind)).map((k) => k.kind)) {
      if (askedModels.has(kind)) continue;
      askedModels.add(kind);
      props.ws
        .call<AgentModel[]>("agent_models", { kind })
        .then((models) => {
          if (Array.isArray(models) && models.length) setListedModels((all) => ({ ...all, [kind]: models }));
        })
        // Not installed, or a daemon from before the call: the CLI's default model it is.
        .catch(() => {});
    }
  });
  createEffect(() => {
    for (const t of store.list()) {
      const models = t.agent?.models;
      if (!drivesChat(t.kind) || !models?.length) continue;
      if (JSON.stringify(untrack(listedModels)[t.kind]) !== JSON.stringify(models)) setListedModels((all) => ({ ...all, [t.kind]: models }));
    }
  });

  function openNew() {
    setHistory(false);
    setGear(false);
    setPicking(true);
    queueMicrotask(() => input?.focus());
  }

  async function start(kind: AgentKind, options: AgentOptions, prompt?: string) {
    // A view made first, so the agent starts at the size it will be shown at.
    const view = new TermView(store, (url) => props.ws.backend.openLink(url));
    setStarting({ view, kind: kind.kind });
    setPicking(false);
    setHistory(false);
    for (const other of views.values()) other.el.hidden = true;
    await view.mount(body);
    try {
      const info = await store.start(kind.kind, options, view.size(), prompt);
      views.set(info.id, view);
      view.bind(info.id);
      props.onSelect(info.id);
      setSentAt(Date.now());
      if (kind.kind === "claude" || drivesChat(kind.kind)) input?.focus();
      else view.focus();
    } catch (err) {
      view.dispose();
      notify(`Could not start ${kind.name}: ${message(err)}`);
      setPicking(true);
    } finally {
      setStarting(undefined);
    }
  }

  async function end(terminal: TerminalInfo) {
    setGear(false);
    if (terminal.running) {
      const choice = await ask(
        `End this ${terminal.title} conversation?`,
        ["End conversation", "Cancel"],
        "The agent stops, along with anything it is running, and the conversation leaves the history.",
      );
      if (choice !== "End conversation") return;
    }
    try {
      await store.close(terminal.id);
    } catch (err) {
      notify(`Could not end the conversation: ${message(err)}`);
    }
  }

  /**
   * Sends a message, with what was added to it with @: to Codex's bridge,
   * or as if typed into the agent's terminal, the chips going first so its
   * prompt hook finds them.
   */
  function send(t: TerminalInfo, text: string, refs: MessageRef[] = []): boolean {
    if (chat()?.waiting.length) {
      notify(`Answer ${agentName(t)}'s question above first.`);
      return false;
    }
    setStopped((ids) => ids.filter((id) => id !== t.id));
    const view = views.get(t.id);
    if (driven(t)) {
      tell(t, refs.length ? { kind: "say", text, refs } : { kind: "say", text });
      setSentAt(Date.now());
      return true;
    }
    if (refs.length) tell(t, { kind: "set", refs });
    if (view) {
      view.paste(text);
      // Enter on its own, once the paste has landed.
      setTimeout(() => void store.input(t.id, "\r"), SUBMIT_DELAY_MS);
    } else {
      void store.say(t.id, text);
    }
    setSentAt(Date.now());
    return true;
  }

  /**
   * Starts the agent again on its conversation, with `options`: after it
   * ended, or to change what it can't while it runs. `prompt` goes on with it.
   */
  async function restart(t: TerminalInfo, options: AgentOptions, prompt?: string) {
    const id = t.id;
    setRestarting((ids) => [...ids, id]);
    try {
      await store.restart(id, options, views.get(id)?.size(), prompt);
      const drop = <T,>(all: Record<string, T>) => {
        const { [id]: _, ...rest } = all;
        return rest;
      };
      setModeChoice(drop);
      setEffortChoice(drop);
      setModelChoice(drop);
      setCodeChoice(drop);
      setBoxChoice(drop);
      setStopped((ids) => ids.filter((other) => other !== id));
      setSentAt(Date.now());
    } catch (err) {
      notify(`Could not start ${agentName(t)} again: ${message(err)}`);
    } finally {
      setRestarting((ids) => ids.filter((other) => other !== id));
    }
  }

  /** What an ended conversation goes on with: as it last ran, with what was picked here since. */
  function resumeOptions(t: TerminalInfo): AgentOptions {
    const live = liveOptions(t);
    const model = modelChoice()[t.id];
    return {
      ...live,
      model: model ? model.id : live.model,
      effort: effortChoice()[t.id] ?? live.effort,
      permission: modeChoice()[t.id]?.permission ?? live.permission,
      code: (codeChoice()[t.id] ?? codeTo(t)) === "console" ? "console" : undefined,
      ...boxChoice()[t.id],
    };
  }

  /** Claude Code turns YOLO on only as it starts: it starts again on this conversation in YOLO mode. */
  async function restartInYolo(t: TerminalInfo, yolo: Mode) {
    const go = "Restart in YOLO";
    const choice = await ask(
      `Restart ${agentName(t)} in YOLO mode?`,
      [go, "Cancel"],
      `Claude Code turns YOLO on only as it starts, so it starts again and goes on with this conversation${working(t) ? ", stopping what it is doing now" : ""}. From then on, everything runs without asking.`,
    );
    if (choice === go) await restart(t, { ...liveOptions(t), permission: yolo.permission });
  }

  /** An outdated agent starts again on its conversation with the newest tools, if it can; else a new one starts. */
  async function renew(t: TerminalInfo) {
    if (!canResume(t)) return start(agentKind(t.kind)!, t.options ?? {});
    if (working(t)) {
      const go = "Restart";
      const choice = await ask(`Restart ${agentName(t)} now?`, [go, "Cancel"], "It stops what it is doing now, then goes on with this conversation.");
      if (choice !== go) return;
    }
    await restart(t, liveOptions(t));
  }

  function stop(t: TerminalInfo) {
    if (driven(t)) tell(t, { kind: "interrupt" });
    else void store.interrupt(t.id);
    setStopped((ids) => [...ids.filter((id) => id !== t.id), t.id]);
  }

  function tell(t: TerminalInfo, command: Record<string, unknown>) {
    store.tell(t.id, command).catch((err) => notify(`Could not reach ${agentName(t)}: ${message(err)}`));
  }

  function compact(t: TerminalInfo) {
    if (driven(t)) tell(t, { kind: "compact" });
    else command(t, "/compact");
  }

  function answer(t: TerminalInfo, a: Ask, reply: Answer) {
    setSent((ids) => new Set(ids).add(a.id));
    store.answer(t.id, a.id, reply).catch((err) => {
      notify(`Could not answer: ${message(err)}`);
      setSent((ids) => {
        const next = new Set(ids);
        next.delete(a.id);
        return next;
      });
    });
  }

  /** Types a command, then Enter; while the agent waits for an answer, it would answer it, so it waits. */
  let held: (() => void) | undefined;
  createEffect(
    on(
      () => current()?.agent?.state === "waiting",
      (waiting) => {
        if (waiting || !held) return;
        const run = held;
        held = undefined;
        run();
      },
    ),
  );
  function command(t: TerminalInfo, text: string) {
    const run = () => {
      void store.input(t.id, text);
      setTimeout(() => void store.input(t.id, "\r"), SUBMIT_DELAY_MS);
    };
    if (t.agent?.state === "waiting") held = run;
    else run();
  }
  let effortTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(effortTimer));
  function changeEffort(t: TerminalInfo, level: string) {
    setEffortChoice((all) => ({ ...all, [t.id]: level }));
    clearTimeout(effortTimer);
    effortTimer = setTimeout(() => {
      if (driven(t)) tell(t, { kind: "set", effort: level });
      else command(t, `/effort ${level}`);
    }, EFFORT_SETTLE_MS);
  }

  /**
   * Switches Codex's mode for its next turn, or Claude Code's as ⇧Tab does,
   * until its terminal shows the mode. Restricted and Ask are Sandhill's,
   * on top of Claude Code's default mode: the daemon is told which is on.
   */
  async function switchMode(t: TerminalInfo, mode: Mode) {
    if (driven(t)) {
      setModeChoice((all) => ({ ...all, [t.id]: mode }));
      tell(t, { kind: "set", permission: mode.permission });
      return;
    }
    const view = views.get(t.id);
    if (!view) return;
    for (let i = 0; i <= MODE_TRIES; i++) {
      const shownMode = screenMode(view.screenText());
      if (!shownMode) break;
      if (shownMode.claude === mode.claude) {
        setModeChoice((all) => ({ ...all, [t.id]: mode }));
        tell(t, { kind: "set", permission: mode.permission });
        return;
      }
      if (i === MODE_TRIES) break;
      await store.input(t.id, "\x1b[Z");
      await new Promise((done) => setTimeout(done, MODE_STEP_MS));
    }
    const back = screenMode(view.screenText());
    if (back) setModeChoice((all) => ({ ...all, [t.id]: back }));
    notify(`Claude Code didn't switch to ${mode.label} mode here. Its terminal (gear menu) shows the modes it offers: ⇧Tab switches.`);
  }

  /** Renames in the history's list, which has no field of its own. */
  async function rename(t: TerminalInfo) {
    setGear(false);
    const name = await askText("Rename conversation", title(t), "Rename", "Leave it empty to name it after its first message again.");
    if (name === undefined) return;
    await saveName(t, name);
  }

  async function saveName(t: TerminalInfo, name: string) {
    if (name.trim() === title(t)) return;
    try {
      await store.rename(t.id, name.trim());
    } catch (err) {
      notify(`Could not rename the conversation: ${err instanceof Error ? err.message : err}`);
    }
  }

  /** The message box's choices for a conversation: as the agent keeps them, or as picked here while it has ended. */
  const choices = (t: TerminalInfo) => ({ ...liveOptions(t), ...boxChoice()[t.id] });
  /** Changes a choice: the agent takes it from its next message, or it waits for the conversation to start again. */
  function choose(t: TerminalInfo, choice: Pick<AgentOptions, "web" | "context">) {
    if (!t.running) {
      setBoxChoice((all) => ({ ...all, [t.id]: { ...all[t.id], ...choice } }));
      return;
    }
    const command: Record<string, unknown> = { kind: "set" };
    if ("web" in choice) command.web = choice.web !== false;
    if ("context" in choice) command.context = choice.context !== false;
    tell(t, command);
  }

  /** What the @ menu offers: the project's files, the front document's chunks, the sessions' variables and recent plots. */
  async function mentionList(): Promise<Mention[]> {
    const sessions = props.ws.projectSessions().filter((s) => s.status === "running");
    const [files, variables, plots] = await Promise.all([
      props.ws.call<string[]>("list_files", { path: props.project }).catch(() => [] as string[]),
      Promise.all(
        sessions.map((s) =>
          props.ws
            .call<Variables>("list_variables", { session: s.id })
            .then((v) => v.variables.map((variable): Mention => ({ ref: { kind: "variable", name: variable.name, session: s.id, language: s.language } })))
            .catch(() => [] as Mention[]),
        ),
      ),
      Promise.all(
        sessions.map((s) =>
          props.ws
            .call<SessionPlots>("session_plots", { session: s.id, limit: PLOTS_OFFERED })
            .then((p) => p.plots.flatMap((plot) => fromJournal(s.id, plot) ?? []))
            .catch(() => [] as PlotItem[]),
        ),
      ),
    ]);
    const doc = props.active;
    const chunks: Mention[] = doc
      ? doc.codeCells().map((cell) => ({ ref: { kind: "chunk", name: `${doc.name} › ${cell.name}`, path: relative(doc.path), cell: cell.id } }))
      : [];
    const recent = plots
      .flat()
      .sort((a, b) => b.tsMs - a.tsMs)
      .slice(0, PLOTS_OFFERED);
    return [
      ...files.map((file): Mention => ({ ref: { kind: "file", name: file, path: file } })),
      ...chunks,
      ...variables.flat(),
      ...recent.map((plot): Mention => {
        const name = plotName(plot, relative);
        return { ref: { kind: "plot", name }, make: () => savePlot(plot, name) };
      }),
    ];
  }

  /** Saves a plot where the agent can look at it, in the project's attachments folder. */
  async function savePlot(plot: PlotItem, name: string): Promise<MessageRef> {
    const svg = plot.mime === "image/svg+xml";
    const file = `${ATTACHMENTS}/plot-${plot.run.slice(0, 8)}.${svg ? "svg" : (plot.mime.split("/")[1] ?? "png").replace("jpeg", "jpg")}`;
    await saveAttachment(file, await imageData(props.ws, plot), !svg);
    return { kind: "plot", name, path: file };
  }

  /** A file from this Mac, saved in the project's attachments folder for the agent to read. */
  async function attachFile(file: File): Promise<MessageRef> {
    const name = attachmentName(file.name, new Date());
    const path = `${ATTACHMENTS}/${name}`;
    await saveAttachment(path, toBase64(new Uint8Array(await file.arrayBuffer())), true);
    return { kind: "attach", name, path };
  }

  /** Writes into the attachments folder, which keeps itself out of git. */
  async function saveAttachment(path: string, text: string, base64: boolean) {
    await props.ws.call("write_file", { path: documentPath(props.project, path), text, base64 });
    await props.ws.call("write_file", { path: documentPath(props.project, `${ATTACHMENTS}/.gitignore`), text: "*\n" });
  }

  /** Saves the conversation as a file in the project, under a name the user can change. */
  async function exportChat(t: TerminalInfo, format: ExportFormat) {
    setGear(false);
    const label = EXPORT_FORMATS.find((f) => f.format === format)?.label ?? format;
    const name = (await askText(`Export as ${label}`, exportName(title(t), format), "Export", `Saved in the project's folder on ${props.hostLabel}.`))?.trim();
    if (!name) return;
    const path = documentPath(props.project, name);
    try {
      const [stat] = await props.ws.call<FileStat[]>("stat_files", { paths: [path] });
      if (stat?.mtime_ms !== undefined) {
        const choice = await ask(`Replace ${relative(path)}?`, ["Replace", "Cancel"], "A file with this name is already there.");
        if (choice !== "Replace") return;
      }
      const text = exportConversation(format, store.chat(t.id), {
        title: title(t),
        agent: agentName(t),
        kind: t.kind,
        model: modelLabel(t),
        documents: t.options?.documents ?? [],
        at: new Date(),
      });
      await props.ws.call("write_file", { path, text });
      notify(`Exported the conversation to ${relative(path)}.`, "info");
    } catch (err) {
      notify(`Could not export the conversation: ${err instanceof Error ? err.message : err}`);
    }
  }

  async function insert(t: TerminalInfo, code: string, language?: string) {
    const file = t.options?.documents?.[0];
    const path = file ? documentPath(props.project, file) : props.active?.path;
    if (!path) {
      notify("Open a document to insert the code into.");
      return;
    }
    const doc = await props.onOpen(path);
    if (!doc) return;
    try {
      doc.addCell(code, language, doc.place().cell);
      await doc.save();
    } catch (err) {
      notify(`Could not insert the code: ${message(err)}`);
    }
  }

  const openPath = (path: string) => void props.onOpen(documentPath(props.project, path));
  const othersWaiting = () => store.waiting().some((t) => t.id !== current()?.id);

  return (
    <div
      class="pane agents-pane"
      classList={{ min: props.min, max: props.max }}
      style={{ width: `${props.width}px` }}
      aria-label="Agents"
    >
      <div class="tabs agent-head" role="tablist">
        <div class="cap tabcap">
        <div class="tab active chat-title" role="tab" aria-selected="true">
          <Switch fallback={<span class="atl">Agents</span>}>
            <Match when={history()}>
              <Icon name="clock" small />
              <span class="atl">History</span>
            </Match>
            <Match when={picking() || starting()}>
              <Icon name="plus" small />
              <span class="atl">New conversation</span>
              <Show when={picking() && store.agents().length}>
                <button class="ax" title="Cancel (esc)" onClick={() => setPicking(false)}>
                  <Icon name="x" small />
                </button>
              </Show>
            </Match>
            <Match when={current()}>
              {(t) => (
                <>
                  <ProviderIcon kind={t().kind} />
                  <Show
                    when={renaming() === t().id}
                    fallback={
                      <span class="atl" title={`${tabTitle(t())} · Double-click to rename`} onDblClick={() => setRenaming(t().id)}>
                        {title(t())}
                      </span>
                    }
                  >
                    <TitleField
                      value={title(t())}
                      onDone={(name) => {
                        const term = t();
                        setRenaming(undefined);
                        if (name !== undefined) void saveName(term, name);
                      }}
                    />
                  </Show>
                </>
              )}
            </Match>
          </Switch>
        </div>
        </div>
        <span class="spacer" />
        <button class="pane-button" title="New conversation" onClick={openNew}>
          <Icon name="plus" small />
        </button>
        <button
          class="pane-button hist-btn"
          classList={{ on: history() }}
          aria-pressed={history()}
          title={othersWaiting() ? "Conversation history: another conversation is waiting for you" : "Conversation history"}
          onClick={() => {
            setPicking(false);
            setGear(false);
            setHistory(!history());
          }}
        >
          <Icon name="clock" small />
          <Show when={othersWaiting()}>
            <span class="dot wait" />
          </Show>
        </button>
        <button class="pane-button gear-btn" title="Conversation settings" disabled={!current() || history() || picking()} onClick={() => setGear(!gear())}>
          <Icon name="gear" small />
        </button>
        <PaneButtons pane="agents" min={props.min} max={props.max} onMin={props.onMin} onMax={props.onMax} />
      </div>
      <div class="agent-body">
        <Show when={gear() && current()}>
          {(t) => (
            <div class="apop gear">
              <Show when={canChat(t())}>
                <button
                  class="mm-i"
                  onClick={() => {
                    // Read before closing the menu, which unmounts `t`.
                    const shown = t().id;
                    setGear(false);
                    setTermShown((ids) => (ids.includes(shown) ? ids.filter((id) => id !== shown) : [...ids, shown]));
                  }}
                >
                  <Icon name={chatMode(t()) ? "command" : "chat"} small />
                  <span>{chatMode(t()) ? "Show terminal" : "Back to chat"}</span>
                </button>
              </Show>
              <Show when={t().running && (t().kind === "claude" || driven(t()))}>
                <button
                  class="mm-i"
                  onClick={() => {
                    const term = t();
                    setGear(false);
                    compact(term);
                  }}
                >
                  <Icon name="restart" small />
                  <span>Compact conversation</span>
                  <kbd>/compact</kbd>
                </button>
              </Show>
              <div class="mm-sep" />
              <Show when={canChat(t())}>
                <For each={EXPORT_FORMATS}>
                  {(f) => (
                    <button class="mm-i" onClick={() => void exportChat(t(), f.format)}>
                      <Icon name="export" small />
                      <span>Export as {f.label}</span>
                    </button>
                  )}
                </For>
              </Show>
              <button
                class="mm-i"
                onClick={() => {
                  const id = t().id;
                  setGear(false);
                  setRenaming(id);
                }}
              >
                <Icon name="edits" small />
                <span>Rename</span>
              </button>
              <div class="mm-sep" />
              <button class="mm-i" onClick={() => void end(t())}>
                <Icon name="trash" small />
                <span>{t().running ? "End conversation" : "Remove from history"}</span>
              </button>
            </div>
          )}
        </Show>
        <Show when={history()}>
          <History
            store={store}
            current={current()?.id}
            hostLabel={props.hostLabel}
            title={title}
            onOpen={(t) => {
              setHistory(false);
              props.onSelect(t.id);
            }}
            onEnd={(t) => void end(t)}
            onRename={(t) => void rename(t)}
          />
        </Show>
        <Show when={picking()}>
          <NewConversation
            store={store}
            hostLabel={props.hostLabel}
            project={baseName(props.project)}
            onInstall={props.onInstall}
            choice={newChoice}
            onCancel={store.agents().length ? () => setPicking(false) : undefined}
          />
        </Show>
        <Show when={!picking() && !history() && !current() && !starting()}>
          <div class="agent-empty">
            <div class="ae-ico">
              <Icon name="chat" />
            </div>
            <b>No conversations</b>
            <p>
              Chat with Claude Code, Codex or Antigravity about this project. They run on {props.hostLabel} with your own plan, can run code
              in your live R and Python sessions, and add cells to your files.
            </p>
            <button class="primary" onClick={openNew}>
              <Icon name="plus" small />
              New conversation
            </button>
          </div>
        </Show>
        <Show when={!picking() && !history() && current()?.running && current()?.outdated && !keptOld().includes(current()!.id) && current()}>
          {(t) => (
            <Outdated terminal={t()} resumable={canResume(t())} onAgain={(o) => void renew(o)} onKeep={(o) => setKeptOld((ids) => [...ids, o.id])} />
          )}
        </Show>
        <Show when={!picking() && !history() && !starting() && current() && !chatMode(current()!) && current()}>
          {(t) => (
            <div class="termbar">
              <Icon name="command" small />
              <span class="tb-l">
                {agentKind(t().kind)?.program ?? t().kind} in a terminal on {props.hostLabel} · {modelLabel(t())}
              </span>
              <span class="spacer" />
              <Show when={canChat(t())}>
                <button onClick={() => setTermShown((ids) => ids.filter((id) => id !== t().id))}>
                  <Icon name="chat" small />
                  Back to chat
                </button>
              </Show>
            </div>
          )}
        </Show>
        <Show when={!picking() && !history() && (chatStarting() || (!starting() && current() && chatMode(current()!)))}>
          <AgentChat
            chat={(!starting() && chat()) || { items: [], waiting: [] }}
            agent={agentName(current())}
            sent={sent()}
            sentAt={sentAt()}
            onAnswer={(a, reply) => answer(current()!, a, reply)}
            onOpen={openPath}
            onLink={(url) => props.ws.backend.openLink(url)}
            onInsert={(code, language) => void insert(current()!, code, language)}
            linked={current()?.options?.documents?.[0]}
          >
            <ChatNotes
              terminal={starting() ? undefined : current()}
              agentName={starting() ? kindName(starting()!.kind) : undefined}
              local={props.ws.backend.host === LOCAL_HOST}
              onLogin={(t, method) => tell(t, { kind: "login", method })}
              onLink={(url) => props.ws.backend.openLink(url)}
              starting={!!starting() || restarting().includes(current()?.id ?? "")}
              resumed={!!current() && store.chat(current()!.id).length > sinceStart(current()!)}
              entries={current() ? sinceStart(current()!) : 0}
              waitingCards={chat()?.waiting.length ?? 0}
              now={now()}
              trust={/Yes, I trust this folder/.test(screen())}
              onTrust={(t) => {
                setScreen("");
                // The second choice of Claude Code's own question.
                void store.input(t.id, "\x1b[B").then(() => setTimeout(() => void store.input(t.id, "\r"), TRUST_KEY_MS));
              }}
              onShowTerminal={(t) => setTermShown((ids) => [...ids, t.id])}
            />
          </AgentChat>
        </Show>
        <div
          class="term-host"
          ref={body}
          classList={{ behind: chatStarting() || (!starting() && !!current() && chatMode(current()!)) }}
          hidden={picking() || history() || (!current() && !starting())}
        />
        <Show when={!picking() && !history() && current() && !current()!.running && !restarting().includes(current()!.id) && current()}>
          {(t) => (
            <Ended
              terminal={t()}
              resumable={canResume(t())}
              onClose={() => void end(t())}
              onAgain={(o) => void start(agentKind(o.kind)!, o.options ?? {})}
              onResume={(o) => void restart(o, resumeOptions(o))}
            />
          )}
        </Show>
      </div>
      <Show when={picking()}>
        <NewComposer
          choice={newChoice}
          mentions={mentionList}
          onAttach={attachFile}
          ref={(el) => (input = el)}
          onSend={(kind, options, text) => {
            void start(kind, options, text);
            return true;
          }}
          fill={fill()?.key === "new" ? fill() : undefined}
          onFilled={() => setFill(undefined)}
        />
      </Show>
      <Show when={!picking() && !history() && !starting() && current() && (current()!.running || resumable(current()!)) && current()}>
        {(t) => {
          const kind = () => withModels(agentKind(t().kind)!, t());
          const mode = () => modeChoice()[t().id] ?? currentMode(t());
          const waiting = () => t().agent?.state === "waiting";
          /** Ended: what is picked here applies as the message starts it again. */
          const ended = () => !t().running;
          const modelNote = () => {
            if (ended()) return `Applies as ${agentName(t())} goes on with your next message.`;
            if (driven(t())) return working(t()) ? "Changes apply from your next message." : undefined;
            return waiting() ? `Changes here wait until you answer ${agentName(t())}.` : undefined;
          };
          return (
            <AgentComposer
              draftKey={t().id}
              fill={fill()?.key === t().id ? fill() : undefined}
              onFilled={() => setFill(undefined)}
              kind={kind()}
              kinds={AGENT_KINDS.map((k) => (k.kind === t().kind ? kind() : k))}
              locked
              model={modelChoice()[t().id]?.label ?? modelLabel(t())}
              isModel={(k, m) => {
                if (k !== t().kind) return false;
                const picked = modelChoice()[t().id];
                return picked ? picked.id === m.id : currentModel(t(), m);
              }}
              effort={effortChoice()[t().id] ?? effortLevel(t())}
              effortNote={t().kind === "claude" || driven(t()) ? undefined : "Codex changes effort in its own /model menu"}
              mode={mode()}
              modeNote={(m) => {
                if (ended() || driven(t())) return undefined;
                if (t().kind !== "claude") return m.id === mode().id ? undefined : "Codex changes this with /approvals in its terminal";
                if (waiting()) return m.id === mode().id ? undefined : `Answer ${agentName(t())} first`;
                if (m.id === "yolo" && mode().id !== "yolo" && !canResume(t())) return "Start a new conversation in YOLO mode";
                return undefined;
              }}
              web={choices(t()).web !== false}
              onWeb={(on) => choose(t(), { web: on ? undefined : false })}
              context={choices(t()).context !== false}
              onContext={(on) => choose(t(), { context: on ? undefined : false })}
              mentions={mentionList}
              onAttach={attachFile}
              status={t().agent?.status}
              busy={working(t())}
              modelNote={modelNote()}
              ref={(el) => (input = el)}
              onSend={(text, refs) => {
                if (!ended()) return send(t(), text, refs);
                void restart(t(), { ...resumeOptions(t()), refs: refs.length ? refs : undefined }, text);
                return true;
              }}
              onStop={() => stop(t())}
              onModel={(_, m: ModelChoice) => {
                if (ended()) {
                  setModelChoice((all) => ({ ...all, [t().id]: m }));
                } else if (driven(t())) {
                  setModelChoice((all) => ({ ...all, [t().id]: m }));
                  tell(t(), { kind: "set", model: m.id ?? null });
                } else if (t().kind === "claude" && m.id) {
                  setModelChoice((all) => ({ ...all, [t().id]: m }));
                  command(t(), `/model ${m.id}`);
                } else {
                  setTermShown((ids) => [...ids, t().id]);
                  command(t(), "/model");
                }
              }}
              onEffort={(level) => (ended() ? setEffortChoice((all) => ({ ...all, [t().id]: level })) : changeEffort(t(), level))}
              onMode={(m) => {
                if (ended()) setModeChoice((all) => ({ ...all, [t().id]: m }));
                else if (t().kind === "claude" && m.id === "yolo" && mode().id !== "yolo") void restartInYolo(t(), m);
                else void switchMode(t(), m);
              }}
              codeTo={codeChoice()[t().id] ?? codeTo(t())}
              onCodeTo={(to) => {
                if (ended()) setCodeChoice((all) => ({ ...all, [t().id]: to }));
                else tell(t(), { kind: "set", code: to === "console" ? "console" : null });
              }}
              onCompact={() => compact(t())}
            />
          );
        }}
      </Show>
    </div>
  );
}

/** What a new conversation will be: the agent, its model and effort, and its mode. */
interface NewChoice {
  kind: () => AgentKind;
  /** Every agent, with the models those that list theirs listed. */
  kinds: () => AgentKind[];
  setKind: (kind: string) => void;
  model: () => ModelChoice;
  setModel: (model: ModelChoice) => void;
  effort: () => string | undefined;
  setEffort: (level: string) => void;
  mode: () => Mode;
  setMode: (mode: Mode) => void;
  code: () => CodeTo;
  setCode: (to: CodeTo) => void;
  web: () => boolean;
  setWeb: (on: boolean) => void;
  context: () => boolean;
  setContext: (on: boolean) => void;
}

const [newKind, setNewKind] = createSignal("claude");
const [newModel, setNewModel] = createSignal<Record<string, ModelChoice>>({});
/** The models Codex and Antigravity listed, by agent: asked as a new conversation opens, or as a conversation reports them. */
const [listedModels, setListedModels] = createSignal<Record<string, AgentModel[]>>({});
const newKindOf = (kind: AgentKind) => listedKind(kind, listedModels()[kind.kind], newModel()[kind.kind]?.id);
const [newEffort, setNewEffort] = createSignal<Record<string, string>>({});
const [newMode, setNewMode] = createSignal<Mode>(MODES[0]);
const [newCode, setNewCode] = createSignal<CodeTo>("file");
const [newWeb, setNewWeb] = createSignal(true);
const [newContext, setNewContext] = createSignal(true);
const newChoice: NewChoice = {
  kind: () => newKindOf(agentKind(newKind()) ?? AGENT_KINDS[0]),
  kinds: () => AGENT_KINDS.map(newKindOf),
  setKind: setNewKind,
  model: () => {
    const kind = newChoice.kind();
    return newModel()[newKind()] ?? kind.models[kind.start] ?? kind.models[0];
  },
  setModel: (model) => setNewModel((all) => ({ ...all, [newKind()]: model })),
  effort: () => newEffort()[newKind()],
  setEffort: (level) => setNewEffort((all) => ({ ...all, [newKind()]: level })),
  mode: () => newMode(),
  setMode: setNewMode,
  code: () => newCode(),
  setCode: setNewCode,
  web: newWeb,
  setWeb: setNewWeb,
  context: newContext,
  setContext: setNewContext,
};

/** New conversation (draft 34): what it works in; the message box under it picks the agent and starts it. */
function NewConversation(props: {
  store: TerminalStore;
  hostLabel: string;
  project: string;
  choice: NewChoice;
  onInstall: (command: string) => void;
  onCancel?: () => void;
}) {
  const [installed] = createResource(() => props.store.listAgents().catch(() => [] as AgentInfo[]));
  const info = (kind: string) => installed()?.find((a) => a.kind === kind);
  const kind = () => props.choice.kind();
  const missing = () => installed() && !info(kind().kind)?.path;
  return (
    <div class="newchat">
      <div class="nc-in">
        <div class="nc-h">
          <b>New conversation</b>
          <Show when={props.onCancel}>
            <button class="nc-cancel" title="Back to the conversation you were in (esc)" onClick={() => props.onCancel?.()}>
              Cancel
            </button>
          </Show>
        </div>
        <p>
          It works on the whole {props.project} project and sees your live sessions. To have it work on a file, type @ and pick the file, or a
          chunk, variable or plot.
        </p>
        <p class="nc-hint">
          Choose the agent, model and thinking effort from{" "}
          <b>
            {shortModel(props.choice.model().label)} ({effortName(props.choice.effort() ?? kind().recommended)}) ⌄
          </b>{" "}
          in the message box.
        </p>
        <Show when={missing()}>
          <div class="pk-install">
            <span>
              {kind().name} isn't installed on {props.hostLabel}. Install it with <code>{info(kind().kind)?.install}</code>
            </span>
            <button class="primary" onClick={() => props.onInstall(info(kind().kind)!.install)}>
              Install in a terminal
            </button>
          </div>
        </Show>
        <p class="nc-sign">
          Runs on {props.hostLabel} in this project's folder, signed in with your own {kind().name} account.
        </p>
      </div>
    </div>
  );
}

function NewComposer(props: {
  choice: NewChoice;
  mentions: () => Promise<Mention[]>;
  onAttach: (file: File) => Promise<MessageRef>;
  ref: (el: HTMLTextAreaElement) => void;
  onSend: (kind: AgentKind, options: AgentOptions, text: string) => boolean;
  fill?: { text: string; refs: MessageRef[] };
  onFilled: () => void;
}) {
  const c = props.choice;
  return (
    <AgentComposer
      draftKey="new"
      fill={props.fill}
      onFilled={props.onFilled}
      kind={c.kind()}
      kinds={c.kinds()}
      model={c.model().label}
      isModel={(kind, m) => kind === c.kind().kind && m.id === c.model().id}
      effort={c.effort()}
      mode={modeFor(c.kind(), c.mode())}
      web={c.web()}
      onWeb={c.setWeb}
      context={c.context()}
      onContext={c.setContext}
      mentions={props.mentions}
      onAttach={props.onAttach}
      ref={props.ref}
      onSend={(text, refs) =>
        props.onSend(
          c.kind(),
          {
            model: c.model().id,
            effort: c.effort(),
            permission: modeFor(c.kind(), c.mode()).permission,
            code: c.code() === "console" ? "console" : undefined,
            web: c.web() ? undefined : false,
            context: c.context() ? undefined : false,
            refs: refs.length ? refs : undefined,
          },
          text,
        )
      }
      onModel={(kind, model) => {
        if (kind.kind !== c.kind().kind) {
          // Another agent keeps the effort if it has that level, else takes its own default.
          const effort = c.effort();
          c.setKind(kind.kind);
          if (effort && kind.efforts.includes(effort)) c.setEffort(effort);
        }
        c.setModel(model);
      }}
      onEffort={c.setEffort}
      onMode={c.setMode}
      codeTo={c.code()}
      onCodeTo={c.setCode}
    />
  );
}

/** Notes at the end of the chat: starting, signing in, or waiting on something only the terminal shows. */
function ChatNotes(props: {
  terminal?: TerminalInfo;
  /** The agent's name, for one starting. */
  agentName?: string;
  /** The agent runs on this Mac, where a browser sign-in can come back to it. */
  local: boolean;
  onLogin: (t: TerminalInfo, method: "browser" | "code") => void;
  onLink: (url: string) => void;
  starting: boolean;
  /** Entries since the agent last started; `resumed` when it went on with a conversation. */
  entries: number;
  resumed: boolean;
  waitingCards: number;
  now: number;
  /** Claude Code asks whether to trust the project's folder, as it does the first time. */
  trust: boolean;
  onTrust: (t: TerminalInfo) => void;
  onShowTerminal: (t: TerminalInfo) => void;
}) {
  const name = () => props.agentName ?? agentName(props.terminal);
  const stuck = () => {
    const t = props.terminal;
    if (!t?.running) return false;
    if (t.agent?.state === "waiting" && !props.waitingCards) return true;
    return !props.entries && props.now - (t.started_ms ?? t.created_ms) > START_GRACE_MS;
  };
  return (
    <Switch>
      <Match when={props.terminal?.running && props.terminal.agent?.auth_required && props.terminal.agent.account === null && props.terminal}>
        {(t) => (
          <SignIn
            name={name()}
            login={t().agent?.login}
            local={props.local}
            onLogin={(method) => props.onLogin(t(), method)}
            onLink={props.onLink}
          />
        )}
      </Match>
      <Match when={props.trust && !props.entries && props.terminal}>
        {(t) => (
          <div class="approve" role="group" aria-label="Trust this folder?">
            <div class="ap-top">
              <Icon name="hand" />
              <span>
                <b>Trust this folder?</b>
                <small>
                  The first time in a folder, Claude Code asks whether to trust it. It can then read, edit and run files here.
                </small>
              </span>
            </div>
            <div class="ap-btns">
              <button class="apb pri" onClick={() => props.onTrust(t())}>
                Trust this folder
              </button>
              <span class="spacer" />
              <button class="apb" onClick={() => props.onShowTerminal(t())}>
                Show terminal
              </button>
            </div>
          </div>
        )}
      </Match>
      <Match when={stuck() && props.terminal}>
        {(t) => (
          <div class="chat-stuck">
            <span>
              {props.entries
                ? `${name()} is asking something in its terminal.`
                : props.resumed
                  ? `${name()} hasn't picked up the conversation yet. It may be asking something in its terminal.`
                  : `${name()} hasn't started the conversation. It may be asking something in its terminal, such as whether to trust this folder.`}
            </span>
            <button onClick={() => props.onShowTerminal(t())}>
              <Icon name="command" small />
              Show terminal
            </button>
          </div>
        )}
      </Match>
      <Match when={props.starting || (props.terminal?.running && !props.entries)}>
        <div class="thinking">
          <span class="spin" />
          Starting {name()}…
        </div>
      </Match>
    </Switch>
  );
}

/**
 * Codex isn't signed in: it signs in with ChatGPT in a browser, which comes
 * back to it on this Mac only, or with a code entered on any device. Codex
 * keeps the sign-in; Sandhill only shows where to go.
 */
function SignIn(props: {
  name: string;
  login?: AgentReport["login"];
  local: boolean;
  onLogin: (method: "browser" | "code") => void;
  onLink: (url: string) => void;
}) {
  // The browser opens once Codex says where.
  let opening = false;
  createEffect(
    on(
      () => props.login?.url,
      (url) => {
        if (url && opening && !props.login?.code) props.onLink(url);
        if (url) opening = false;
      },
    ),
  );
  const browser = () => {
    opening = true;
    props.onLogin("browser");
  };
  return (
    <div class="approve" role="group" aria-label={`Sign in to ${props.name}`}>
      <div class="ap-top">
        <Icon name="hand" />
        <span>
          <b>Sign in to {props.name}</b>
          <small>{props.name} needs your ChatGPT account. It keeps the sign-in itself; Sandhill never sees it.</small>
        </span>
      </div>
      <Show when={props.login?.code ? props.login : undefined}>
        {(login) => (
          <p class="ap-n">
            Go to{" "}
            <button class="flink" onClick={() => props.onLink(login().url!)}>
              {login().url}
            </button>{" "}
            and enter the code <code>{login().code}</code>
          </p>
        )}
      </Show>
      <Show when={props.login?.url && !props.login.code}>
        <p class="ap-n">
          Finish signing in in your browser.{" "}
          <button class="flink" onClick={() => props.onLink(props.login!.url!)}>
            Open the page again
          </button>
        </p>
      </Show>
      <Show when={props.login?.error}>{(error) => <p class="ap-n err">{error()}</p>}</Show>
      <div class="ap-btns">
        <Show
          when={props.local}
          fallback={
            <button class="apb pri" onClick={() => props.onLogin("code")}>
              Sign in with a code
            </button>
          }
        >
          <button class="apb pri" onClick={browser}>
            Sign in with ChatGPT
          </button>
          <button class="apb" title="Sign in on another device, with a code" onClick={() => props.onLogin("code")}>
            Use a code
          </button>
        </Show>
      </div>
    </div>
  );
}

/** The project's conversations: those waiting for you, running, then by day. */
function History(props: {
  store: TerminalStore;
  current?: string;
  hostLabel: string;
  title: (t: TerminalInfo) => string;
  onOpen: (t: TerminalInfo) => void;
  onEnd: (t: TerminalInfo) => void;
  onRename: (t: TerminalInfo) => void;
}) {
  const [search, setSearch] = createSignal("");
  const rows = createMemo(() => {
    const q = search().trim().toLowerCase();
    return props.store
      .agents()
      .map((t) => ({ t, title: props.title(t), last: lastActive(props.store.chat(t.id)) ?? t.exit?.ended_ms ?? t.created_ms }))
      .filter((r) => !q || r.title.toLowerCase().includes(q) || (r.t.options?.documents ?? []).some((d) => d.toLowerCase().includes(q)))
      .sort((a, b) => b.last - a.last);
  });
  const day = (ms: number) => {
    const d = new Date(ms);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  };
  const today = day(Date.now());
  const groups = createMemo(() => {
    const all = rows();
    const state = (t: TerminalInfo) => (t.running ? t.agent?.state : undefined);
    const rest = all.filter((r) => state(r.t) !== "waiting" && state(r.t) !== "busy");
    return [
      { name: "Needs you", rows: all.filter((r) => state(r.t) === "waiting") },
      { name: "Running", rows: all.filter((r) => state(r.t) === "busy") },
      { name: "Today", rows: rest.filter((r) => day(r.last) === today) },
      { name: "Yesterday", rows: rest.filter((r) => day(r.last) === today - 86_400_000) },
      { name: "Earlier", rows: rest.filter((r) => day(r.last) < today - 86_400_000) },
    ].filter((g) => g.rows.length);
  });
  const detail = (t: TerminalInfo, last: number) => {
    if (!t.running) return `ended · ${when(last)}`;
    if (t.agent?.state === "waiting") return "waiting for you";
    if (t.agent?.state === "busy") return "running";
    return when(last);
  };
  return (
    <div class="hist">
      <label class="hsearch2">
        <Icon name="search" small />
        <input placeholder="Search conversations" value={search()} onInput={(e) => setSearch(e.currentTarget.value)} />
      </label>
      <Show when={groups().length} fallback={<p class="hnone">{search() ? "No conversations match." : "No conversations yet."}</p>}>
        <For each={groups()}>
          {(group) => (
            <>
              <div class="hgrp">{group.name}</div>
              <For each={group.rows}>
                {(row) => (
                  <div
                    class="hrow"
                    classList={{ on: row.t.id === props.current }}
                    role="button"
                    tabindex="0"
                    onClick={(e) => {
                      if (!(e.target as Element).closest(".hr-acts")) props.onOpen(row.t);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") props.onOpen(row.t);
                    }}
                  >
                    <span class="hr-ic">
                      <ProviderIcon kind={row.t.kind} />
                      <Show when={row.t.running && (row.t.agent?.state === "waiting" || row.t.agent?.state === "busy")}>
                        <span class="dot" classList={{ wait: row.t.agent?.state === "waiting", busy: row.t.agent?.state === "busy" }} />
                      </Show>
                    </span>
                    <span class="hr-t">
                      <b>{row.title}</b>
                      <small>
                        <Show when={row.t.options?.documents?.[0]}>
                          {(doc) => (
                            <>
                              <FileIcon name={doc()} />
                              {doc()} ·{" "}
                            </>
                          )}
                        </Show>
                        {modelLabel(row.t)} · {detail(row.t, row.last)}
                      </small>
                    </span>
                    <span class="hr-acts">
                      <button title="Rename" onClick={() => props.onRename(row.t)}>
                        <Icon name="edits" small />
                      </button>
                      <button title={row.t.running ? "End conversation" : "Remove from history"} onClick={() => props.onEnd(row.t)}>
                        <Icon name="trash" small />
                      </button>
                    </span>
                  </div>
                )}
              </For>
            </>
          )}
        </For>
      </Show>
      <p class="hfoot">
        Conversations are kept by sandhilld on {props.hostLabel}, and keep running when you close the window. Ended ones stay
        here until you remove them.
      </p>
    </div>
  );
}

/** The conversation's title as a field: Enter or leaving it keeps the name, Esc keeps the old one. */
function TitleField(props: { value: string; onDone: (name: string | undefined) => void }) {
  let field!: HTMLInputElement;
  let done = false;
  const finish = (name: string | undefined) => {
    if (done) return;
    done = true;
    props.onDone(name?.trim() ? name : undefined);
  };
  onMount(() => {
    field.focus();
    field.select();
  });
  return (
    <input
      ref={field}
      class="arename"
      value={props.value}
      aria-label="Conversation title"
      spellcheck={false}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          finish(field.value);
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          finish(undefined);
        }
      }}
      onBlur={() => finish(field.value)}
    />
  );
}

/** "umap · analysis.qmd" for a cell's plot, "Console plot · 14:31" for one from the Console. */
function plotName(plot: PlotItem, relative: (path: string) => string): string {
  if (plot.cell) return `${plot.cell.cell_id} · ${relative(plot.cell.file)}`;
  const time = new Date(plot.tsMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return `${plot.by ? `${plot.by}'s plot` : "Console plot"} · ${time}`;
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

/** "Claude", "Codex", "Antigravity". */
function agentName(terminal: TerminalInfo | undefined): string {
  if (!terminal) return "Claude";
  return agentKind(terminal.kind) ? kindName(terminal.kind) : terminal.title;
}

function kindName(kind: string): string {
  return kind === "claude" ? "Claude" : (agentKind(kind)?.name ?? kind);
}

/** The agent's program ended: it can go on with the conversation, if it said which one it is, or start a new one. */
function Ended(props: {
  terminal: TerminalInfo;
  resumable: boolean;
  onClose: () => void;
  onAgain: (t: TerminalInfo) => void;
  onResume: (t: TerminalInfo) => void;
}) {
  const how = () => {
    const exit = props.terminal.exit;
    if (exit?.signal) return `was stopped (signal ${exit.signal})`;
    if (exit?.code) return `ended with exit code ${exit.code}`;
    return "ended";
  };
  return (
    <div class="agent-ended" role="status">
      <span>
        {props.terminal.title} {how()}.{props.resumable ? " Resume it, or send a message, to go on with this conversation." : ""}
      </span>
      <span class="spacer" />
      <Show when={props.resumable} fallback={<button onClick={() => props.onAgain(props.terminal)}>Start again</button>}>
        <button title="Start the agent again on this conversation" onClick={() => props.onResume(props.terminal)}>
          Resume
        </button>
      </Show>
      <button onClick={props.onClose}>Remove</button>
    </div>
  );
}

/**
 * A session started before Sandhill was updated. The agent keeps the tools
 * it started with, so it can't use what the update added (such as writing
 * cells into the document) until it starts again: on this conversation if
 * it can, else as a new one.
 */
function Outdated(props: {
  terminal: TerminalInfo;
  resumable: boolean;
  onAgain: (t: TerminalInfo) => void;
  onKeep: (t: TerminalInfo) => void;
}) {
  return (
    <div class="agent-ended agent-outdated" role="status">
      <span>This conversation started before Sandhill was updated, so it doesn't have the newest tools.</span>
      <span class="spacer" />
      <button
        title={props.resumable ? "Start the agent again on this conversation, with the newest tools" : undefined}
        onClick={() => props.onAgain(props.terminal)}
      >
        {props.resumable ? "Restart" : "Start a new one"}
      </button>
      <button title="Keep using this conversation" onClick={() => props.onKeep(props.terminal)}>
        Keep
      </button>
    </div>
  );
}

/** Whether `model` is the one the session runs: as the agent reports it, else as it started. */
function currentModel(terminal: TerminalInfo, model: ModelChoice): boolean {
  // Codex says which of the models it listed.
  if (terminal.agent?.models?.length) return model.id === terminal.agent.status?.model_id;
  const reported = terminal.agent?.status?.model;
  if (reported) return reported.toLowerCase().includes(model.label.toLowerCase());
  return terminal.options?.model === model.id;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function baseName(path: string): string {
  return path.split("/").pop() ?? path;
}
