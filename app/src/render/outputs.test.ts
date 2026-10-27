import { describe, expect, it, vi } from "vitest";
import { OutputArea, needsFrame, renderOutput } from "./outputs";

const ctx = {
  blobUrl: (hash: string, mime: string) => `blob-test://${hash}?${mime}`,
  blobText: vi.fn(async (hash: string) => `<b>${hash}</b>`),
  onResize: vi.fn(),
};

describe("renderOutput", () => {
  it("shows streams with stderr marked", () => {
    const node = renderOutput({ output_type: "stream", name: "stderr", text: "warn\r100%\n" }, ctx);
    expect(node.classList.contains("out-stderr")).toBe(true);
    expect(node.textContent).toBe("100%\n");
  });

  it("prefers rich MIME types over plain text", () => {
    const node = renderOutput(
      { output_type: "display_data", data: { "text/plain": "<Figure>", "image/png": "iVBO\nRw==\n" }, metadata: {} },
      ctx,
    ) as HTMLImageElement;
    expect(node.tagName).toBe("IMG");
    expect(node.src).toBe("data:image/png;base64,iVBORw==");
  });

  it("serves blob-backed images by URL", () => {
    const node = renderOutput(
      { output_type: "display_data", data: { "image/png": { sandhill_blob: "abc", size: 9 } }, metadata: {} },
      ctx,
    ) as HTMLImageElement;
    expect(node.getAttribute("src")).toBe("blob-test://abc?image/png");
  });

  it("renders static HTML inline and sanitized", () => {
    const node = renderOutput(
      { output_type: "execute_result", data: { "text/html": "<table><tr><td>1</td></tr></table><img src=x onerror=alert(1)>" }, metadata: {} },
      ctx,
    );
    expect(node.classList.contains("out-frame")).toBe(true);
    const plain = renderOutput(
      { output_type: "execute_result", data: { "text/html": "<table><tr><td>1</td></tr></table>" }, metadata: {} },
      ctx,
    );
    expect(plain.shadowRoot?.querySelector("td")?.textContent).toBe("1");
  });

  it("shows the first rows of a long table until asked for all", () => {
    const rows = Array.from({ length: 250 }, (_, i) => `<tr><td>${i}</td></tr>`).join("");
    const node = renderOutput(
      { output_type: "execute_result", data: { "text/html": `<table><tbody>${rows}</tbody></table>` }, metadata: {} },
      ctx,
    );
    const root = node.shadowRoot!;
    expect(root.querySelectorAll("tr")).toHaveLength(200);
    const more = root.querySelector("button")!;
    expect(more.textContent).toBe("Show all 250 rows");
    more.click();
    expect(root.querySelectorAll("tr")).toHaveLength(250);
    expect(root.querySelector("tr:last-child td")?.textContent).toBe("249");
    expect(root.querySelector("button")).toBeNull();
  });

  it("sends scripted HTML to a sandboxed frame", () => {
    expect(needsFrame('<div id="w"></div><script>render()</script>')).toBe(true);
    expect(needsFrame("<table></table>")).toBe(false);
  });

  it("draws plotly figures sent as data in a frame", () => {
    const node = renderOutput(
      { output_type: "display_data", data: { "application/vnd.plotly.v1+json": { data: [{ type: "bar" }], layout: {} }, "text/plain": "Figure" }, metadata: {} },
      ctx,
    );
    expect(node.classList.contains("out-frame")).toBe(true);
  });

  it("leaves outputs another pane shows to a line saying so", () => {
    const elsewhere = (data: Record<string, unknown>) => {
      if (!("text/html" in data)) return undefined;
      const note = document.createElement("div");
      note.textContent = "in the Viewer";
      return note;
    };
    const page = renderOutput({ output_type: "display_data", data: { "text/html": "<script>x</script>" }, metadata: {} }, { ...ctx, elsewhere });
    expect(page.textContent).toBe("in the Viewer");
    const text = renderOutput({ output_type: "execute_result", data: { "text/plain": "1" }, metadata: {} }, { ...ctx, elsewhere });
    expect(text.textContent).toBe("1");
  });

  it("shows tracebacks", () => {
    const node = renderOutput(
      { output_type: "error", ename: "ValueError", evalue: "x", traceback: ["\x1b[31mValueError\x1b[0m: x"] },
      ctx,
    );
    expect(node.textContent).toBe("ValueError: x");
  });

  it("loads large text values from blobs", async () => {
    const node = renderOutput(
      { output_type: "display_data", data: { "text/html": { sandhill_blob: "big", size: 1e6 } }, metadata: {} },
      ctx,
    );
    const parent = document.createElement("div");
    parent.append(node);
    await vi.waitFor(() => expect(parent.firstElementChild?.shadowRoot?.textContent).toContain("big"));
  });
});

describe("OutputArea", () => {
  it("keeps unchanged outputs and replaces changed ones", () => {
    const area = new OutputArea(ctx);
    const a = { output_type: "stream", name: "stdout", text: "a" };
    const b = { output_type: "stream", name: "stderr", text: "b" };
    area.update([a, b]);
    const first = area.element.children[0];
    area.update([a, { ...b, text: "bb" }]);
    expect(area.element.children[0]).toBe(first);
    expect(area.element.children[1].textContent).toBe("bb");
    area.update([]);
    expect(area.element.children).toHaveLength(0);
  });
});
