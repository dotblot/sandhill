import { describe, expect, it } from "vitest";
import type { TerminalInfo } from "../backend/types";
import { formatDuration, isJobScript, jobNote, jobState } from "./jobs";

function job(extra: Partial<TerminalInfo>): TerminalInfo {
  return { id: "j", kind: "job", title: "align.sh", cwd: "/p", created_ms: 0, started_ms: 0, running: false, end: 0, ...extra };
}

describe("jobs", () => {
  it("knows the scripts it can run", () => {
    expect(["/p/de.R", "/p/qc.py", "/p/a.sh", "/p/main.nf", "/p/rules.smk", "/p/Snakefile"].every(isJobScript)).toBe(true);
    expect(["/p/notes.md", "/p/data.csv", "/p/report.qmd"].some(isJobScript)).toBe(false);
  });

  it("says how long and how a job ended", () => {
    expect(formatDuration(45_000)).toBe("45s");
    expect(formatDuration(12 * 60_000 + 5_000)).toBe("12m");
    expect(formatDuration((2 * 60 + 13) * 60_000)).toBe("2h 13m");
    expect(formatDuration(26 * 3_600_000)).toBe("1d 2h");

    const ended = (exit: TerminalInfo["exit"]) => job({ exit });
    expect(jobState(job({ running: true }))).toBe("running");
    expect(jobNote(ended({ code: 0, ended_ms: 3_000 }))).toBe("Job align.sh finished after 3s.");
    expect(jobNote(ended({ code: 3, ended_ms: 90_000 }))).toBe("Job align.sh failed (exit 3) after 1m.");
    expect(jobNote(ended({ signal: 1, ended_ms: 60_000 }))).toBe("Job align.sh was stopped after 1m.");
  });
});
