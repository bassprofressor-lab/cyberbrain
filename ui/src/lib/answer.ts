import type { Conflict, Hit } from "@/api/client";

/**
 * Which hit the simple view presents as the answer.
 *
 * The first hit, unless the recall itself says a rule above it wins, or the note has been
 * replaced: a hit that lost a conflict (`Conflict.loser`) or whose note is `superseded_by`
 * another is the wrong thing to read out as "the answer", however well it scored. The next
 * hit that is neither takes its place. When every hit is one of those, the first hit is still
 * shown, because an answer with a warning beats none, and the screen says what is wrong.
 */
export function pickAnswer(hits: Hit[], conflicts: Conflict[]): { answer: Hit | undefined; index: number } {
  const losers = new Set(conflicts.map((c) => c.loser));
  const index = hits.findIndex((h) => !losers.has(h.citation) && !h.superseded_by && !h.invalid_at);
  const at = index < 0 ? 0 : index;
  return { answer: hits[at], index: hits.length ? at : -1 };
}
