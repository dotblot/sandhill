import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import { AGENT_KINDS, type CodeTo, codeTo, MODES } from "../model/agents";
import type { TerminalInfo } from "../backend/types";
import { AgentComposer } from "./AgentComposer";

describe("AgentComposer", () => {
  it("shows a new model and effort as soon as they change", () => {
    const root = document.body.appendChild(document.createElement("div"));
    const [model, setModel] = createSignal("GPT-5.5");
    const [effort, setEffort] = createSignal<string | undefined>("medium");
    const dispose = render(
      () => (
        <AgentComposer
          kind={AGENT_KINDS[1]!}
          model={model()}
          isModel={() => false}
          effort={effort()}
          mode={MODES[0]!}
          onSend={() => true}
          onModel={() => {}}
          onEffort={setEffort}
          onMode={() => {}}
        />
      ),
      root,
    );
    const chip = () => root.querySelector(".cchip.model")?.textContent;
    expect(chip()).toBe("GPT-5.5 (Medium)");
    setEffort("high");
    expect(chip()).toBe("GPT-5.5 (High)");
    setModel("GPT-6-Sol");
    expect(chip()).toBe("GPT-6-Sol (High)");
    // No effort picked: the recommended one.
    setEffort(undefined);
    expect(chip()).toBe("GPT-6-Sol (Medium)");
    dispose();
    root.remove();
  });

  it("switches where the agent's code goes under the modes", () => {
    const root = document.body.appendChild(document.createElement("div"));
    const [to, setTo] = createSignal<CodeTo>("file");
    const picked: CodeTo[] = [];
    const dispose = render(
      () => (
        <AgentComposer
          kind={AGENT_KINDS[0]!}
          model="Opus"
          isModel={() => false}
          mode={MODES[0]!}
          codeTo={to()}
          onCodeTo={(next) => {
            picked.push(next);
            setTo(next);
          }}
          onSend={() => true}
          onModel={() => {}}
          onEffort={() => {}}
          onMode={() => {}}
        />
      ),
      root,
    );
    (root.querySelector(".cbi.mode") as HTMLButtonElement).click();
    const buttons = () => [...root.querySelectorAll<HTMLButtonElement>(".seg2 button")];
    expect(buttons().map((b) => [b.textContent, b.getAttribute("aria-pressed")])).toEqual([
      ["In the file", "true"],
      ["In the Console", "false"],
    ]);
    buttons()[1].click();
    buttons()[1].click();
    expect(picked).toEqual(["console"]);
    expect(buttons()[1].getAttribute("aria-pressed")).toBe("true");
    dispose();
    root.remove();
  });

  it("reads where code goes from the agent's report, then from how it started", () => {
    const terminal = (options: TerminalInfo["options"], agent?: TerminalInfo["agent"]) => ({ options, agent }) as TerminalInfo;
    expect(codeTo(terminal({}))).toBe("file");
    expect(codeTo(terminal({ code: "console" }))).toBe("console");
    expect(codeTo(terminal({ code: "console" }, { code: null }))).toBe("file");
    expect(codeTo(terminal({}, { code: "console" }))).toBe("console");
  });
});
