import { describe, expect, it } from "vitest";
import type { ChatEntry, ChatEvent } from "./chat";
import { exportHtml, exportJson, exportMarkdown, exportName, type ExportInfo } from "./export";

const info: ExportInfo = {
  title: "Markers for <PBMC>",
  agent: "Claude",
  kind: "claude",
  model: "Opus 5.5",
  documents: ["analysis.qmd"],
  at: new Date(2026, 9, 18, 9, 30),
};

function entries(events: ChatEvent[]): ChatEntry[] {
  return events.map((event, seq) => ({ seq, event: { at_ms: seq, ...event } }));
}

const PNG = "iVBORw0KGgo=";

const conversation = entries([
  { kind: "session", source: "startup" },
  { kind: "prompt", turn: "p1", text: "Plot the markers" },
  { kind: "text", turn: "p1", message: "m1", index: 0, text: "Plotting them now." },
  { kind: "step", turn: "p1", id: "t1", tool: "mcp__sandhill__run_code", input: { code: "plot(markers)", language: "r" } },
  {
    kind: "step_done",
    turn: "p1",
    id: "t1",
    output: [
      { type: "text", text: "Ran in R session 1a2b (R 4.4.1) · ok · 0.5 s\n[1] 9 clusters" },
      { type: "image", data: PNG, mimeType: "image/png" },
    ],
  },
  { kind: "step", turn: "p1", id: "t2", tool: "Bash", input: { command: "rm -rf results" } },
  { kind: "ask", turn: "p1", id: "a1", tool: "Bash", input: { command: "rm -rf results" } },
  { kind: "answered", id: "a1", answer: { behavior: "deny" } },
  { kind: "text", turn: "p1", message: "m2", index: 0, text: "Done. <script>alert(1)</script>" },
  { kind: "stop", turn: "p1" },
  { kind: "session", source: "resume" },
]);

describe("exporting a conversation", () => {
  it("writes Markdown with each message, step and its output", () => {
    const md = exportMarkdown(conversation, info);
    expect(md.startsWith("# Markers for <PBMC>\n\nClaude · Opus 5.5 · analysis.qmd")).toBe(true);
    expect(md).toContain("## You\n\nPlot the markers\n\n## Claude\n\nPlotting them now.");
    expect(md).toContain("**`run_code`** plot(markers) · R · 0.5 s\n\n```r\nplot(markers)\n```\n\n```\n[1] 9 clusters\n```");
    expect(md).toContain("*1 plot, in the HTML export*");
    expect(md).toContain("**`bash`** rm -rf results · you said no\n\n```sh\nrm -rf results\n```");
    expect(md).toContain("*Conversation resumed*");
    // One heading per speaker, not per reply.
    expect(md.match(/## Claude/g)).toHaveLength(1);
  });

  it("writes a standalone HTML page with the plots", () => {
    const html = exportHtml(conversation, info);
    expect(html).toContain("<title>Markers for &lt;PBMC&gt;</title>");
    expect(html).toContain(`<img src="data:image/png;base64,${PNG}" alt="Plot">`);
    expect(html).toContain("<h2>You</h2>");
    expect(html).toContain('<code class="language-r">plot(markers)');
    // What the agent wrote can't run in the page.
    expect(html).not.toContain("<script>alert");
  });

  it("writes JSON with every entry the daemon kept", () => {
    const json = JSON.parse(exportJson(conversation, info));
    expect(json).toMatchObject({ sandhill: "conversation", version: 1, agent: "claude", documents: ["analysis.qmd"] });
    expect(json.entries).toEqual(conversation);
  });

  it("names the file after the conversation", () => {
    expect(exportName("Markers for <PBMC>", "md")).toBe("markers-for-pbmc.md");
    expect(exportName("Clustering — résumé", "html")).toBe("clustering-resume.html");
    expect(exportName("???", "json")).toBe("conversation.json");
  });
});
