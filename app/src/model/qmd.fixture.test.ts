import { describe, expect, it } from "vitest";
import cases from "../../../tests/fixtures/chunks.json";
import { parseChunks } from "./qmd";

// The daemon finds chunks too (crates/daemon/src/qmd.rs), for agents' edits
// to documents no window has open; both read these cases.
describe("chunks shared with the daemon", () => {
  for (const c of cases) {
    it(c.name, () => {
      const chunks = parseChunks(c.text).map((k) => ({
        cell_id: k.cellId,
        language: k.language,
        code: k.code,
        open_line: k.openLine,
        close_line: k.closeLine,
      }));
      expect(chunks).toEqual(c.chunks);
    });
  }
});
