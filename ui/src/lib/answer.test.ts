import { describe, expect, it } from "vitest";
import type { Conflict, Hit } from "@/api/client";
import { answerParts, isSearchHint, pickAnswer } from "./answer";

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

describe("search-hint lines (O1)", () => {
  it("recognises a block that is only a *Für:* / *For:* line", () => {
    expect(isSearchHint("*Für: zahlungsziel · skonto · angebot*")).toBe(true);
    expect(isSearchHint("  *For: payment terms · discount*  \n")).toBe(true);
    expect(isSearchHint("*Für: a*\n*Für: b*")).toBe(true);
    expect(isSearchHint("Skonto gilt 14 Tage.\n*Für: skonto*")).toBe(false);
    expect(isSearchHint("")).toBe(false);
  });
  it("does not pick a search-hint block as the answer when another hit exists", () => {
    const r = pickAnswer([hit("hint", { text: "*Für: zahlungsziel · skonto · angebot*" }), hit("real", { text: "Skonto gilt 14 Tage." })], []);
    expect(r.answer?.citation).toBe("real");
    expect(r.index).toBe(1);
  });
  it("still shows a hint block when nothing else is there", () => {
    expect(pickAnswer([hit("hint", { text: "*Für: x*" })], []).answer?.citation).toBe("hint");
  });
});

describe("answerParts (O1)", () => {
  it("sets a leading heading apart as the title and keeps the rest as markdown", () => {
    expect(answerParts("## Zahlungsziel\n\nNetto **14 Tage**, Skonto 2 %.")).toEqual({ title: "Zahlungsziel", body: "Netto **14 Tage**, Skonto 2 %." });
  });
  it("has no title when the block does not start with a heading", () => {
    expect(answerParts("Netto 14 Tage.")).toEqual({ title: null, body: "Netto 14 Tage." });
  });
  it("drops *Für:* lines from the body", () => {
    expect(answerParts("# Skonto\n*Für: skonto · rabatt*\n\nZwei Prozent.")).toEqual({ title: "Skonto", body: "Zwei Prozent." });
  });
});
