import { describe, expect, it } from "vitest";
import type { KernelspecInfo } from "../backend/types";
import { pickKernel, wantedEnvs } from "./workspace";

const specs: KernelspecInfo[] = [
  { name: "ark", display_name: "Ark R Kernel", language: "R" },
  { name: "conda-env-omicverse-py", display_name: "Python (omicverse)", language: "python", env: "omicverse" },
  { name: "conda-env-scanpy-py", display_name: "Python (scanpy)", language: "python", env: "scanpy" },
  { name: "ir", display_name: "R", language: "R" },
  { name: "python3", display_name: "Python (base)", language: "python", env: "base" },
];

const pick = (language: string, wanted?: { name?: string; display_name?: string }) =>
  pickKernel(specs, language, wanted)?.name;

describe("pickKernel", () => {
  it("defaults to python3 and Ark", () => {
    expect(pick("python")).toBe("python3");
    expect(pick("python3")).toBe("python3");
    expect(pick("r")).toBe("ark");
  });

  it("takes the kernel a notebook names", () => {
    expect(pick("python", { name: "conda-env-scanpy-py", display_name: "Python (scanpy)" })).toBe(
      "conda-env-scanpy-py",
    );
    expect(pick("r", { name: "ir", display_name: "R" })).toBe("ir");
  });

  it("finds the environment VS Code recorded as the display name", () => {
    expect(pick("python", { name: "python3", display_name: "omicverse" })).toBe("conda-env-omicverse-py");
  });

  it("finds environments named the nb_conda_kernels way", () => {
    expect(pick("python", { name: "conda-env-omicverse-python3", display_name: "Python 3" })).toBe(
      "conda-env-omicverse-py",
    );
    expect(pick("python", { name: "conda-env-old-py", display_name: "Python [conda env:scanpy]" })).toBe(
      "conda-env-scanpy-py",
    );
    expect(pick("python", { name: "conda-root-py", display_name: "Python [conda root]" })).toBe("python3");
  });

  it("falls back to the default when the environment is gone", () => {
    expect(pick("python", { name: "python3", display_name: "Python 3 (ipykernel)" })).toBe("python3");
    expect(pick("python", { name: "conda-env-gone-py", display_name: "gone" })).toBe("python3");
  });
});

describe("wantedEnvs", () => {
  it("handles environment names with dashes", () => {
    expect(wantedEnvs({ name: "conda-env-sc-rna-2024-py" })).toEqual(["sc-rna-2024"]);
  });
});
