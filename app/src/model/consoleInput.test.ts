import { describe, expect, it } from "vitest";
import { InputHistory, isComplete } from "./consoleInput";

describe("isComplete", () => {
  it("runs whole R statements and waits for the rest of others", () => {
    for (const code of ["x <- 1", "head(df)", "f(a, b) # note", "if (x) print(1)", "s <- 'a(b'", "x |> f()", "`my var` <- 2", ""]) {
      expect(isComplete(code, "r"), code).toBe(true);
    }
    for (const code of ["f(a,", "x <-", "df |>", "df %>%", "a +", "x <- c(1, 2,", "s <- 'abc", "function(x)", "if (x > 1)", "\\(x)", "x <- {", "y ~", "if (a) b else"]) {
      expect(isComplete(code, "r"), code).toBe(false);
    }
  });

  it("runs whole Python statements and waits for blocks to end", () => {
    for (const code of ["x = 1", "print('a:')", "d = {'a': 1}", "for i in range(3):\n    print(i)\n", "'''doc'''", ""]) {
      expect(isComplete(code, "python"), JSON.stringify(code)).toBe(true);
    }
    for (const code of ["for i in range(3):", "def f(x):\n    return x", "x = (1,", "s = '''a", "x = 1 + \\", "@cache", "d = {"]) {
      expect(isComplete(code, "python"), JSON.stringify(code)).toBe(false);
    }
  });
});

describe("InputHistory", () => {
  it("goes back through earlier code and forward to what was typed", () => {
    const history = new InputHistory(["a", "b"]);
    expect(history.older("draft")).toBe("b");
    expect(history.older("b")).toBe("a");
    expect(history.older("a")).toBeUndefined();
    expect(history.newer()).toBe("b");
    expect(history.newer()).toBe("draft");
    expect(history.newer()).toBeUndefined();
    history.push("c\n");
    history.push("c");
    expect(history.entries).toEqual(["a", "b", "c"]);
    expect(history.older("")).toBe("c");
  });
});
