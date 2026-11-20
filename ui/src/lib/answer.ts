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
// The line a note carries for the search ("*Für: zahlungsziel · skonto*", see the cyberbrain
// write convention). It is vocabulary for the index, not something to read out as an answer.
const HINT_LINE = /^\s*\*(?:Für|For):[^\n]*\*\s*$/i;

/** Whether the block is nothing but search-hint lines. */
export function isSearchHint(text: string): boolean {
  const lines = text.split("\n").filter((l) => l.trim() !== "");
  return lines.length > 0 && lines.every((l) => HINT_LINE.test(l));
}

/**
 * The answer block as the screen shows it: a leading heading set apart as the title, search
 * hint lines removed, the rest left as markdown for the renderer.
 */
export function answerParts(text: string): { title: string | null; body: string } {
  const lines = text.split("\n").filter((l) => !HINT_LINE.test(l));
  let body = lines.join("\n").trim();
  const m = /^#{1,6}[ \t]+([^\n]+?)[ \t]*(?:\n|$)/.exec(body);
  if (!m || !m[1]) return { title: null, body };
  body = body.slice(m[0].length).trim();
  return { title: m[1], body };
}

export function pickAnswer(hits: Hit[], conflicts: Conflict[]): { answer: Hit | undefined; index: number } {
  const losers = new Set(conflicts.map((c) => c.loser));
  const index = hits.findIndex((h) => !losers.has(h.citation) && !h.superseded_by && !h.invalid_at && !isSearchHint(h.text));
  const at = index < 0 ? 0 : index;
  return { answer: hits[at], index: hits.length ? at : -1 };
}
