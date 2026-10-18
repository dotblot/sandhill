import { describe, expect, it } from "vitest";
import type { TerminalInfo } from "../backend/types";
import {
  agentKind,
  canResume,
  currentMode,
  documentPath,
  effortLabel,
  effortName,
  liveOptions,
  modelLabel,
  planUsage,
  screenMode,
  timeLeft,
  tokens,
  withModels,
} from "./agents";

const terminal = (extra: Partial<TerminalInfo>): TerminalInfo => ({
  id: "a",
  kind: "claude",
  title: "Claude Code",
  cwd: "/p",
  created_ms: 0,
  running: true,
  end: 0,
  ...extra,
});

describe("agents", () => {
  it("names effort levels, preferring what the agent reports", () => {
    expect(effortName("xhigh")).toBe("Extra high");
    expect(effortName("max")).toBe("Max");
    expect(effortLabel(terminal({ options: { effort: "low" } }))).toBe("Low");
    expect(effortLabel(terminal({ options: { effort: "low" }, agent: { status: { effort: "max" } } as TerminalInfo["agent"] }))).toBe("Max");
    expect(effortLabel(terminal({}))).toBeUndefined();
  });

  it("shows the permission mode the agent reports, else the one it started in", () => {
    expect(currentMode(terminal({ options: { permission: "edits" } })).label).toBe("Edits");
    expect(currentMode(terminal({ options: { permission: "supervised" }, agent: { mode: "auto" } })).label).toBe("Auto");
    expect(currentMode(terminal({ agent: { mode: "plan" } })).label).toBe("Plan");
    expect(currentMode(terminal({ agent: { mode: "manual" } })).label).toBe("Normal");
    expect(currentMode(terminal({ options: { permission: "full" }, agent: { mode: "someday" } })).label).toBe("YOLO");
    // Codex's bridge reports the mode it is in, as it was started with it.
    expect(currentMode(terminal({ kind: "codex", options: { permission: "plan" }, agent: { permission: "supervised" } })).label).toBe("Normal");
    // Sandhill's own modes, on top of Claude Code's default.
    expect(currentMode(terminal({ options: { permission: "ask" } })).label).toBe("Ask");
    expect(currentMode(terminal({ options: { permission: "supervised" }, agent: { mode: "default", guard: "restricted" } })).label).toBe("Restricted");
    expect(currentMode(terminal({ options: { permission: "ask" }, agent: { mode: "default", guard: null } })).label).toBe("Normal");
  });

  it("starts again with what the agent runs with now, on the conversation it reported", () => {
    const changed = terminal({
      options: { model: "opus", effort: "medium", permission: "supervised", documents: ["qc.ipynb"] },
      agent: { mode: "plan", session_id: "s1", status: { model_id: "claude-opus-5-5[1m]", effort: "high" } },
    });
    expect(liveOptions(changed)).toEqual({ model: "claude-opus-5-5[1m]", effort: "high", permission: "plan", documents: ["qc.ipynb"] });
    expect(liveOptions(terminal({ options: { model: "sonnet" } }))).toEqual({ model: "sonnet", effort: undefined, permission: "supervised" });
    expect(liveOptions(terminal({ agent: { mode: "default", guard: "ask" } })).permission).toBe("ask");
    expect(canResume(changed)).toBe(true);
    expect(canResume(terminal({ agent: { thread: "t1" } }))).toBe(false);
    expect(canResume(terminal({ kind: "codex", agent: { thread: "t1" } }))).toBe(true);
    expect(canResume(terminal({ kind: "shell", agent: { session_id: "s1" } }))).toBe(false);
  });

  it("offers the models a running Codex lists, with the efforts of the one it uses", () => {
    const codex = agentKind("codex")!;
    expect(withModels(codex, terminal({ kind: "codex" }))).toBe(codex);
    const models = [
      { id: "gpt-5.5", label: "GPT-5.5", efforts: ["low", "medium", "high", "xhigh"], effort: "medium", default: true },
      { id: "gpt-5.4-mini", label: "GPT-5.4 mini", efforts: ["low", "medium"], effort: "low" },
    ];
    const running = withModels(codex, terminal({ kind: "codex", agent: { models, status: { model_id: "gpt-5.4-mini" } } }));
    expect(running.models).toEqual([
      { id: "gpt-5.5", label: "GPT-5.5" },
      { id: "gpt-5.4-mini", label: "GPT-5.4 mini" },
    ]);
    expect(running).toMatchObject({ efforts: ["low", "medium"], recommended: "low" });
    // Until it says which it uses: its default.
    expect(withModels(codex, terminal({ kind: "codex", agent: { models } }))).toMatchObject({ efforts: models[0]!.efforts, recommended: "medium" });
  });

  it("reads the mode under Claude Code's prompt", () => {
    expect(screenMode("❯ \n⏸ manual mode on · ? for shortcuts")?.id).toBe("normal");
    expect(screenMode("⏸ manual mode on · ? for shortcuts\n⏵⏵ accept edits on (shift+tab to cycle)")?.id).toBe("edits");
    expect(screenMode("⏸ plan mode on (shift+tab to cycle)")?.id).toBe("plan");
    expect(screenMode("⏵⏵ bypass permissions on")?.id).toBe("yolo");
    expect(screenMode("  ? for shortcuts")?.id).toBe("normal");
    expect(screenMode("Do you trust the files in this folder?")).toBeUndefined();
  });

  it("finds a session's documents in its project", () => {
    expect(documentPath("/data/pbmc", "analysis.qmd")).toBe("/data/pbmc/analysis.qmd");
    expect(documentPath("/data/pbmc", "/home/h/notes.qmd")).toBe("/home/h/notes.qmd");
  });

  it("names the model as the agent reports it, else as it was started", () => {
    expect(modelLabel(terminal({ options: { model: "opus" } }))).toBe("Opus");
    expect(modelLabel(terminal({ options: { model: "opus" }, agent: { status: { model: "Opus 5.5" } } }))).toBe("Opus 5.5");
    expect(modelLabel(terminal({ kind: "codex", title: "Codex" }))).toBe("Codex's default model");
  });

  it("reads the plan limits from Claude Code's status line", () => {
    const usage = planUsage({
      rate_limits: {
        five_hour: { used_percentage: 41.6, resets_at: 1_791_780_000 },
        seven_day: { used_percentage: 61, resets_at: "2026-10-19T09:00:00Z" },
      },
    });
    expect(usage).toEqual([
      { label: "5h", percent: 42, resetsAt: 1_791_780_000_000 },
      { label: "Week", percent: 61, resetsAt: Date.parse("2026-10-19T09:00:00Z") },
    ]);
    expect(planUsage({})).toEqual([]);
  });

  it("formats time left and token counts", () => {
    expect(timeLeft(35 * 60_000, 0)).toBe("35m");
    expect(timeLeft(130 * 60_000, 0)).toBe("2h 10m");
    expect(timeLeft((5 * 24 + 1) * 3_600_000, 0)).toBe("5d 1h");
    expect(tokens(200_000)).toBe("200K");
    expect(tokens(1_000_000)).toBe("1M");
  });
});
