import { describe, expect, it } from "vitest";
import { nextFocus } from "./focus";

describe("nextFocus", () => {
  it("wraps forwards and backwards", () => {
    expect(nextFocus(2, 3, false)).toBe(0);
    expect(nextFocus(0, 3, true)).toBe(2);
    expect(nextFocus(0, 3, false)).toBe(1);
    expect(nextFocus(2, 3, true)).toBe(1);
  });
  it("pulls focus in from outside", () => {
    expect(nextFocus(-1, 3, false)).toBe(0);
    expect(nextFocus(-1, 3, true)).toBe(2);
  });
  it("has nowhere to go in an empty dialog", () => {
    expect(nextFocus(-1, 0, false)).toBeNull();
  });
});
