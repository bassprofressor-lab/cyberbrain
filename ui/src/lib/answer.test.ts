import { describe, expect, it } from "vitest";
import type { Conflict, Hit } from "@/api/client";
import { pickAnswer } from "./answer";

const hit = (citation: string, extra: Partial<Hit> = {}): Hit => ({ citation, note_id: "n", note_name: citation, ring: 2, score: 1, text: citation, block_idx: 0, sources: [], ...extra }) as Hit;
const conflict = (winner: string, loser: string): Conflict => ({ winner, loser, reason: "ring" }) as Conflict;

describe("pickAnswer", () => {
  it("takes the first hit when nothing is wrong with it", () => {
    expect(pickAnswer([hit("a"), hit("b")], []).answer?.citation).toBe("a");
  });
  it("skips a hit that lost a conflict", () => {
    const r = pickAnswer([hit("loser"), hit("winner")], [conflict("winner", "loser")]);
    expect(r.answer?.citation).toBe("winner");
    expect(r.index).toBe(1);
  });
  it("skips a replaced note and one that no longer holds", () => {
    const hits = [hit("old", { superseded_by: "new" }), hit("gone", { invalid_at: "2026-01-01T00:00:00Z" }), hit("ok")];
    expect(pickAnswer(hits, []).answer?.citation).toBe("ok");
  });
  it("still shows the first hit when all are flawed, and nothing for no hits", () => {
    expect(pickAnswer([hit("old", { superseded_by: "new" })], []).answer?.citation).toBe("old");
    expect(pickAnswer([], []).answer).toBeUndefined();
  });
});
