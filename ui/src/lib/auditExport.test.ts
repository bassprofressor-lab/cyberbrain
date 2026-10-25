import { describe, expect, it } from "vitest";
import { collectAudit } from "./auditExport";

const pages = (n: number) => async (before?: number) => {
  const at = before ?? n;
  return { rows: [at], next_before: at > 1 ? at - 1 : null };
};

describe("collectAudit", () => {
  it("reports a log it read to the end as complete", async () => {
    const r = await collectAudit(pages(3), 10);
    expect(r).toEqual({ rows: [3, 2, 1], complete: true });
  });
  it("reports a log cut off by the page limit as partial", async () => {
    const r = await collectAudit(pages(50), 4);
    expect(r.rows).toHaveLength(4);
    expect(r.complete).toBe(false);
  });
});
