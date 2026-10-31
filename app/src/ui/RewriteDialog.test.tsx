import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import type { GitRewrite } from "../backend/types";
import { RewriteDialog } from "./RewriteDialog";

afterEach(() => document.body.replaceChildren());

const OWN = {
  base: "a0",
  commits: [
    { sha: "b1", time_ms: 1, message: "Add UMAP" },
    { sha: "c2", time_ms: 2, message: "Fix typo" },
    { sha: "d3", time_ms: 3, message: "Add PCA\n\nOn the scaled data." },
  ],
};

it("reorders the branch's commits and squashes one into the commit below", () => {
  let answer: GitRewrite[] | undefined | "none" = "none";
  const root = document.body.appendChild(document.createElement("div"));
  render(() => <RewriteDialog own={OWN} onDone={(commits) => (answer = commits)} />, root);
  const subjects = () => [...root.querySelectorAll(".rw-subject")].map((e) => e.textContent);
  const row = (subject: string) => [...root.querySelectorAll(".rw-row")].find((r) => r.querySelector(".rw-subject")?.textContent === subject)!;
  const primary = () => root.querySelector(".modal-buttons .primary") as HTMLButtonElement;
  // Newest at the top, and nothing to do yet.
  expect(subjects()).toEqual(["Add PCA", "Fix typo", "Add UMAP"]);
  expect(primary().disabled).toBe(true);
  // The bottom commit has nothing below it to squash into.
  expect((row("Add UMAP").querySelector(".rw-squash input") as HTMLInputElement).disabled).toBe(true);

  // PCA goes to the bottom, oldest; the typo fix folds into UMAP.
  (row("Add PCA").querySelector('[aria-label="Move down"]') as HTMLButtonElement).click();
  (row("Add PCA").querySelector('[aria-label="Move down"]') as HTMLButtonElement).click();
  expect(subjects()).toEqual(["Fix typo", "Add UMAP", "Add PCA"]);
  (row("Fix typo").querySelector(".rw-squash input") as HTMLInputElement).click();
  const box = row("Add UMAP").querySelector("textarea") as HTMLTextAreaElement;
  expect(box.value).toBe("Add UMAP\n\nFix typo");
  box.value = "Add UMAP\n\nWith the typo fixed.";
  box.dispatchEvent(new Event("input", { bubbles: true }));
  expect(primary().textContent).toBe("Squash into 2 commits");
  primary().click();
  expect(answer).toEqual([{ sha: "d3" }, { sha: "b1", message: "Add UMAP\n\nWith the typo fixed." }, { sha: "c2", squash: true }]);
});
