import { describe, expect, it } from "vitest";
import { blockStartLines, firstAtOrAfter, previewOf } from "./blocks";

const body = ["# Title", "", "First paragraph.", "", "## Second", "text", "", "Same words", "", "Same words", "", "Last"].join("\n");

describe("blockStartLines", () => {
  it("finds each block's first line, in order", () => {
    const previews = ["Title", "First paragraph.", "Second", "Same words", "Same words", "Last"];
    expect(blockStartLines(body, previews)).toEqual([1, 3, 5, 8, 10, 12]);
  });

  it("does not match an earlier line twice for repeated text", () => {
    expect(blockStartLines(body, ["Same words", "Same words"])).toEqual([8, 10]);
  });

  it("gives null for a block it cannot place and keeps going", () => {
    expect(blockStartLines(body, ["First paragraph.", "", "no such line", "Last"])).toEqual([3, null, null, 12]);
  });

  it("applies the server's preview rule", () => {
    expect(previewOf("### Heading  ")).toBe("Heading");
    expect(previewOf("x".repeat(100))).toHaveLength(80);
    expect(previewOf("ä".repeat(100))).toHaveLength(80);
  });
});

describe("firstAtOrAfter", () => {
  it("picks the nearest rendered node at or after the block's line", () => {
    expect(firstAtOrAfter([1, 3, 5, 8], 4)).toBe(5);
    expect(firstAtOrAfter([1, 3, 5, 8], 5)).toBe(5);
    expect(firstAtOrAfter([1, 3], 9)).toBeNull();
  });
});
