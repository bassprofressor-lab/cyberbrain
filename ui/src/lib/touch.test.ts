/// <reference types="node" />
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// O6: no screen to measure in node, so this reads the stylesheet: on touch devices the
// shared controls must be at least 44 px (2.75rem) high.
describe("touch targets (O6)", () => {
  const css = readFileSync(new URL("../index.css", import.meta.url), "utf8");
  const block = /@media \(pointer: coarse\)\s*\{([\s\S]*)\}\s*\}\s*$/.exec(css)?.[1] ?? "";
  it("has a coarse-pointer block", () => expect(block).not.toBe(""));
  for (const sel of [".btn", ".btn-sm", ".btn-icon", ".nav-item", ".chip-quiet", ".seg-item"]) {
    it(`raises ${sel} to 44px`, () => {
      const re = new RegExp(`${sel.replace(".", "\\.")}[^{}]*\\{[^}]*min-height:\\s*2\\.75rem`);
      expect(block).toMatch(re);
    });
  }
  it("raises the width of icon buttons", () => expect(block).toMatch(/\.btn-icon[^{}]*\{[^}]*min-width:\s*2\.75rem/));
});
