import { describe, expect, it } from "vitest";
import cases from "../../../tests/fixtures/outputs.json";
import { OutputBuilder } from "./outputs";

describe("OutputBuilder", () => {
  for (const c of cases as any[]) {
    it(c.name, () => {
      const builder = new OutputBuilder();
      for (const event of c.events) builder.push(event.msg_type, event.content);
      expect(builder.outputs).toEqual(c.outputs);
    });
  }

  it("replaces changed outputs instead of mutating them", () => {
    const builder = new OutputBuilder();
    builder.push("stream", { name: "stdout", text: "a" });
    const before = builder.outputs[0];
    builder.push("stream", { name: "stdout", text: "b" });
    expect(builder.outputs[0]).not.toBe(before);
    expect(before.text).toBe("a");
  });
});
