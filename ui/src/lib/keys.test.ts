import { describe, expect, it } from "vitest";
import { scopeAllowed } from "./keys";

describe("scopeAllowed", () => {
  it("lets every scope hear the keyboard when no dialog is open", () => {
    expect(scopeAllowed("notes", 0)).toBe(true);
    expect(scopeAllowed("global", 0)).toBe(true);
  });
  it("silences the page behind an open dialog, except the help overlay", () => {
    expect(scopeAllowed("notes", 1)).toBe(false);
    expect(scopeAllowed("global", 2)).toBe(false);
    expect(scopeAllowed("help", 1)).toBe(true);
  });
});
