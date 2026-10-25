import { describe, expect, it } from "vitest";
import { diffSummary, lineDiff } from "./diff";

describe("lineDiff", () => {
  it("marks what was removed and what was added", () => {
    const d = lineDiff("a\nb\nc", "a\nB\nc\nd")!;
    expect(d.map((l) => `${l.kind}:${l.text}`)).toEqual(["same:a", "del:b", "add:B", "same:c", "add:d"]);
    expect(diffSummary(d)).toEqual({ added: 2, removed: 1 });
  });
  it("sees no change in equal texts", () => {
    expect(diffSummary(lineDiff("x\ny", "x\ny")!)).toEqual({ added: 0, removed: 0 });
  });
  it("refuses very large bodies instead of stalling", () => {
    const big = Array.from({ length: 2500 }, (_, i) => String(i)).join("\n");
    expect(lineDiff(big, big)).toBeNull();
  });
});
