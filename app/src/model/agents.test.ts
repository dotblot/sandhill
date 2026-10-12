import { describe, expect, it } from "vitest";
import type { TerminalInfo } from "../backend/types";
import { currentPermission, documentPath, effortLabel, effortName, modelLabel, planUsage, timeLeft, tokens } from "./agents";

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
    expect(currentPermission(terminal({ options: { permission: "edits" } })).label).toBe("Auto-accept edits");
    expect(currentPermission(terminal({ options: { permission: "supervised" }, agent: { mode: "auto" } })).label).toBe("Auto");
    expect(currentPermission(terminal({ agent: { mode: "plan" } })).label).toBe("Plan");
    expect(currentPermission(terminal({ options: { permission: "full" }, agent: { mode: "someday" } })).label).toBe("Full access");
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
