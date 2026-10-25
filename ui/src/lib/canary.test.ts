import { describe, expect, it } from "vitest";
import { canaryState } from "./canary";

const report = (count: number, hits: number) => ({ summary: { count, hits, last_hit_at: hits ? "2026-10-25T10:00:00Z" : null } });

describe("canaryState", () => {
  it("is idle with nothing planted, not a fault", () => {
    expect(canaryState(report(0, 0))).toBe("idle");
  });
  it("is fine while the decoys sit untouched", () => {
    expect(canaryState(report(3, 0))).toBe("ok");
  });
  it("needs a person once one was used", () => {
    expect(canaryState(report(3, 1))).toBe("danger");
  });
});
