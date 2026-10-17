import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import { AGENT_KINDS, MODES } from "../model/agents";
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
});
