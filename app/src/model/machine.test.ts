import { describe, expect, it } from "vitest";
import { formatBytes, kernelShortName, percentOf, sessionLabel } from "./machine";

describe("machine", () => {
  it("formats sizes the way people read them", () => {
    expect(formatBytes(56)).toBe("56 B");
    expect(formatBytes(41 * 1024 ** 2)).toBe("41 MB");
    expect(formatBytes(2.1 * 1024 ** 3)).toBe("2.1 GB");
    expect(formatBytes(24 * 1024 ** 3)).toBe("24 GB");
    // Just under 1 GB reads better as 0.98 GB than as 1000 MB.
    expect(formatBytes(1000 * 1024 ** 2)).toBe("1.0 GB");
  });

  it("names sessions by language and version", () => {
    expect(sessionLabel({ language: "r", language_version: "4.4.1" })).toBe("R 4.4.1");
    expect(sessionLabel({ language: "python", language_version: "3.12.4" })).toBe("Python 3.12");
    expect(sessionLabel({ language: "python" })).toBe("Python");
    expect(sessionLabel({ language: "julia", language_version: "1.10.2" })).toBe("Julia 1.10.2");
  });

  it("gives a kernel's short name", () => {
    const specs = [
      { name: "ark", display_name: "R (Ark)", language: "r" },
      { name: "omicverse", display_name: "Python 3 (omicverse)", language: "python", env: "omicverse" },
      { name: "python3", display_name: "Python 3", language: "python" },
    ];
    expect(kernelShortName("ark", specs)).toBe("Ark");
    expect(kernelShortName("omicverse", specs)).toBe("omicverse");
    expect(kernelShortName("python3", specs)).toBe("Python 3");
    expect(kernelShortName("gone", specs)).toBe("gone");
  });

  it("rounds percentages", () => {
    expect(percentOf(1, 3)).toBe(33);
    expect(percentOf(5, 0)).toBe(0);
  });
});
