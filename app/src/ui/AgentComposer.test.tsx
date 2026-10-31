import { createSignal, Show } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MessageRef, TerminalInfo } from "../backend/types";
import { AGENT_KINDS, type CodeTo, codeTo, MODES } from "../model/agents";
import type { Mention } from "../model/mentions";
import { AgentComposer, type ComposerProps } from "./AgentComposer";

const settle = () => new Promise((r) => setTimeout(r, 0));

const MENTIONS: Mention[] = [
  { ref: { kind: "file", name: "analysis.qmd", path: "analysis.qmd" } },
  { ref: { kind: "file", name: "data/raw/s1.csv", path: "data/raw/s1.csv" } },
  { ref: { kind: "chunk", name: "analysis.qmd › markers", path: "analysis.qmd", cell: "markers" } },
  { ref: { kind: "variable", name: "seu", session: "r1", language: "r" } },
];

function base(): ComposerProps {
  return {
    draftKey: "c1",
    kind: AGENT_KINDS[0]!,
    kinds: AGENT_KINDS,
    model: "Opus 5.5",
    isModel: () => false,
    mode: MODES[0]!,
    web: true,
    onWeb: () => {},
    context: true,
    onContext: () => {},
    mentions: async () => MENTIONS,
    onAttach: async (file) => ({ kind: "attach", name: file.name, path: `.sandhill/attachments/${file.name}` }),
    onSend: () => true,
    onModel: () => {},
    onEffort: () => {},
    onMode: () => {},
  };
}

function mount(props: () => Partial<ComposerProps>) {
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(() => <AgentComposer {...base()} {...props()} />, root);
  const box = root.querySelector<HTMLTextAreaElement>(".ain")!;
  const type = (text: string) => {
    box.focus();
    box.value = text;
    box.setSelectionRange(text.length, text.length);
    box.dispatchEvent(new InputEvent("input", { bubbles: true }));
  };
  const key = (k: string, init: KeyboardEventInit = {}) => box.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
  const items = () => [...root.querySelectorAll<HTMLButtonElement>(".apop.mention .mm-i")].map((b) => b.textContent);
  const chips = () => [...root.querySelectorAll(".cref span:not(.ico)")].map((s) => s.textContent);
  return { root, box, type, key, items, chips, dispose };
}

describe("AgentComposer", () => {
  afterEach(() => {
    document.body.replaceChildren();
    localStorage.clear();
  });

  it("shows a new model and effort as soon as they change, without the version", () => {
    const [model, setModel] = createSignal("GPT-5.5");
    const [effort, setEffort] = createSignal<string | undefined>("medium");
    const t = mount(() => ({ kind: AGENT_KINDS[1]!, model: model(), effort: effort() }));
    const chip = () => t.root.querySelector(".cchip.model")?.textContent;
    expect(chip()).toBe("GPT-5.5 (Medium)");
    setEffort("high");
    expect(chip()).toBe("GPT-5.5 (High)");
    setModel("Opus 5.5");
    expect(chip()).toBe("Opus (High)");
    // No effort picked: the recommended one.
    setEffort(undefined);
    expect(chip()).toBe("Opus (Medium)");
    t.dispose();
  });

  it("switches where the agent's code goes under the modes", () => {
    const [to, setTo] = createSignal<CodeTo>("file");
    const picked: CodeTo[] = [];
    const t = mount(() => ({
      codeTo: to(),
      onCodeTo: (next: CodeTo) => {
        picked.push(next);
        setTo(next);
      },
    }));
    (t.root.querySelector(".cbi.mode") as HTMLButtonElement).click();
    const buttons = () => [...t.root.querySelectorAll<HTMLButtonElement>(".seg2 button")];
    expect(buttons().map((b) => [b.textContent, b.getAttribute("aria-pressed")])).toEqual([
      ["In the file", "true"],
      ["In the Console", "false"],
    ]);
    buttons()[1].click();
    buttons()[1].click();
    expect(picked).toEqual(["console"]);
    expect(buttons()[1].getAttribute("aria-pressed")).toBe("true");
    t.dispose();
  });

  it("offers only the modes the agent has, as it does them", () => {
    const t = mount(() => ({ kind: AGENT_KINDS.find((k) => k.kind === "antigravity")! }));
    (t.root.querySelector(".cbi.mode") as HTMLButtonElement).click();
    const modes = [...t.root.querySelectorAll<HTMLButtonElement>(".pm-opt")];
    expect(modes.map((b) => b.querySelector("b")?.textContent)).toEqual(["Normal", "YOLO", "Plan"]);
    expect(modes[0].querySelector("small")?.textContent).toBe("Edits files in the project. Commands it would ask about are turned down.");
    t.dispose();
  });

  it("reads where code goes from the agent's report, then from how it started", () => {
    const terminal = (options: TerminalInfo["options"], agent?: TerminalInfo["agent"]) => ({ options, agent }) as TerminalInfo;
    expect(codeTo(terminal({}))).toBe("file");
    expect(codeTo(terminal({ code: "console" }))).toBe("console");
    expect(codeTo(terminal({ code: "console" }, { code: null }))).toBe("file");
    expect(codeTo(terminal({}, { code: "console" }))).toBe("console");
  });

  it("opens the @ menu as @ is typed, and a pick becomes a chip that goes with the message", async () => {
    const sent: [string, MessageRef[]][] = [];
    const t = mount(() => ({ onSend: (text: string, refs: MessageRef[]) => (sent.push([text, refs]), true) }));
    expect(t.box.placeholder).toBe("Ask Claude…");
    t.type("What is in @");
    await settle();
    expect(t.items()).toEqual(["analysis.qmd", "data/raw/s1.csv", "analysis.qmd › markers", "seu", "Attach from this Mac…files, images, PDFs"]);
    expect([...t.root.querySelectorAll(".apop.mention .mm-h")].map((h) => h.textContent)).toEqual(["Files", "Chunks", "Variables"]);
    // What follows the @ filters; Enter picks the first, and the "@se" goes.
    t.type("What is in @se");
    expect(t.items()[0]).toBe("seu");
    t.key("Enter");
    await settle();
    expect(t.root.querySelector(".apop.mention")).toBeNull();
    expect(t.box.value).toBe("What is in ");
    expect(t.chips()).toEqual(["seu"]);
    // Esc closes without a pick, and takes the @ away too.
    t.type("What is in @x");
    t.key("Escape");
    expect(t.box.value).toBe("What is in ");
    expect(t.chips()).toEqual(["seu"]);
    t.type("What is in seu?");
    t.key("Enter");
    expect(sent).toEqual([["What is in seu?", [{ kind: "variable", name: "seu", session: "r1", language: "r" }]]]);
    expect(t.chips()).toEqual([]);
    t.dispose();
  });

  it("shows no @ of its own: typing @ finds a file, and files can be pasted", async () => {
    const t = mount(() => ({}));
    expect(t.root.querySelector(".cbi.at")).toBeNull();
    expect(t.box.placeholder).not.toContain("@");
    t.type("@raw");
    await settle();
    expect(t.items()).toEqual(["data/raw/s1.csv", "Attach from this Mac…files, images, PDFs"]);
    t.key("Enter");
    await settle();
    expect(t.chips()).toEqual(["data/raw/s1.csv"]);
    expect(t.box.value).toBe("");
    // A file pasted into the box becomes a chip too.
    const file = new File(["png"], "gel.png", { type: "image/png" });
    const paste = new Event("paste", { bubbles: true, cancelable: true }) as ClipboardEvent;
    Object.defineProperty(paste, "clipboardData", { value: { files: [file] } });
    t.box.dispatchEvent(paste);
    await settle();
    expect(t.chips()).toEqual(["data/raw/s1.csv", "gel.png"]);
    // Backspace at the start takes the last chip away.
    t.box.setSelectionRange(0, 0);
    t.key("Backspace");
    expect(t.chips()).toEqual(["data/raw/s1.csv"]);
    t.dispose();
  });

  it("is about the whole project: no file to follow or pin", () => {
    const t = mount(() => ({}));
    expect(t.root.querySelector(".atfile")).toBeNull();
    t.dispose();
  });

  it("goes with its conversation without breaking the pane", async () => {
    // As when + is clicked: the box for the conversation in front goes, and
    // keeps its draft under that conversation.
    const [shown, setShown] = createSignal<{ id: string } | undefined>({ id: "c9" });
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <Show when={shown()}>{(t) => <AgentComposer {...base()} draftKey={t().id} />}</Show>, root);
    const box = root.querySelector<HTMLTextAreaElement>(".ain")!;
    box.value = "unsent";
    box.dispatchEvent(new InputEvent("input", { bubbles: true }));
    expect(() => setShown(undefined)).not.toThrow();
    expect(root.querySelector(".ain")).toBeNull();
    setShown({ id: "c9" });
    expect(root.querySelector<HTMLTextAreaElement>(".ain")!.value).toBe("unsent");
    dispose();
  });

  it("lists models by agent, with favourites, and dims other agents once a conversation started", () => {
    const onModel = vi.fn();
    const t = mount(() => ({ locked: true, onModel, isModel: (kind: string, m: { id?: string }) => kind === "claude" && m.id === "claude-opus-5-5" }));
    (t.root.querySelector(".cchip.model") as HTMLButtonElement).click();
    const tabs = () => [...t.root.querySelectorAll<HTMLButtonElement>(".mt-t")];
    const rows = () => [...t.root.querySelectorAll<HTMLElement>(".mt-row")];
    expect(tabs().map((b) => [b.title.split(" · ")[0], b.disabled])).toEqual([
      ["Favourites", false],
      ["Claude Code", false],
      ["Codex", true],
      ["Antigravity", true],
    ]);
    expect(t.root.querySelector(".mt-sub")?.textContent).toBe("Claude Code · your Claude plan");
    expect(rows().map((r) => r.querySelector(".mt-n")?.textContent)).toEqual(["Fable 5.1", "Opus 5.5", "Sonnet 5.5", "Haiku 5.5"]);
    expect(rows()[1].classList.contains("on")).toBe(true);
    expect(rows()[0].querySelector("kbd")?.textContent).toBe("⌥1");
    // ⌥2 picks the second.
    document.dispatchEvent(new KeyboardEvent("keydown", { code: "Digit2", altKey: true, bubbles: true }));
    expect(onModel).toHaveBeenCalledWith(AGENT_KINDS[0], AGENT_KINDS[0]!.models[1]);
    // Starring Fable adds it to the favourites tab, which starts with Opus.
    (rows()[0].querySelector(".mt-star") as HTMLButtonElement).click();
    tabs()[0].click();
    expect(rows().map((r) => r.querySelector(".mt-n")?.textContent)).toEqual(["Fable 5.1", "Opus 5.5"]);
    (rows()[0].querySelector(".mt-star") as HTMLButtonElement).click();
    (rows()[0].querySelector(".mt-star") as HTMLButtonElement).click();
    expect(t.root.querySelector(".mt-empty")?.textContent).toBe("Star a model to keep it here.");
    t.dispose();
  });

  it("keeps each conversation's unsent text and chips", async () => {
    const [key, setKey] = createSignal("d1");
    const t = mount(() => ({ draftKey: key() }));
    t.type("@");
    await settle();
    t.key("Enter");
    await settle();
    t.type("half a thought");
    setKey("d2");
    expect(t.box.value).toBe("");
    expect(t.chips()).toEqual([]);
    setKey("d1");
    expect(t.box.value).toBe("half a thought");
    expect(t.chips()).toEqual(["analysis.qmd"]);
    t.dispose();
  });
});
