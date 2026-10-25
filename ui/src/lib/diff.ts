/**
 * A line diff for the approval screen: what a proposal would change in an existing note.
 *
 * Longest common subsequence over lines, which is plenty for a note and needs no dependency.
 * The table is quadratic, so two bodies of more than `MAX_LINES` lines each are not diffed:
 * `null` tells the caller to show the two texts side by side instead of a wrong or very slow
 * answer.
 */
export type DiffLine = { kind: "same" | "add" | "del"; text: string };

const MAX_LINES = 2000;

export function lineDiff(before: string, after: string): DiffLine[] | null {
  const a = before.split(/\r?\n/);
  const b = after.split(/\r?\n/);
  if (a.length > MAX_LINES || b.length > MAX_LINES) return null;
  const w = b.length + 1;
  const lcs = new Uint32Array((a.length + 1) * w);
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i * w + j] = a[i] === b[j] ? (lcs[(i + 1) * w + j + 1] ?? 0) + 1 : Math.max(lcs[(i + 1) * w + j] ?? 0, lcs[i * w + j + 1] ?? 0);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ kind: "same", text: a[i] ?? "" });
      i++;
      j++;
    } else if ((lcs[(i + 1) * w + j] ?? 0) >= (lcs[i * w + j + 1] ?? 0)) {
      out.push({ kind: "del", text: a[i] ?? "" });
      i++;
    } else {
      out.push({ kind: "add", text: b[j] ?? "" });
      j++;
    }
  }
  for (; i < a.length; i++) out.push({ kind: "del", text: a[i] ?? "" });
  for (; j < b.length; j++) out.push({ kind: "add", text: b[j] ?? "" });
  return out;
}

export function diffSummary(d: DiffLine[]): { added: number; removed: number } {
  return { added: d.filter((l) => l.kind === "add").length, removed: d.filter((l) => l.kind === "del").length };
}
