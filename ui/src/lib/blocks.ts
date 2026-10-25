/**
 * Where each block of a note starts in its body.
 *
 * The server sends a block's index and a one-line preview (its first line, without leading
 * `#`, at most 80 characters), not its position. A citation or a hit says "block 7", and the
 * page used to put every block's anchor in front of the text, so the link landed on the
 * first line whatever the number.
 *
 * Blocks are cut from the body in order and never overlap, so the preview of block N is found
 * by walking the lines forward from where block N-1 was found. A block whose preview cannot be
 * found (it began with a blank line, or the preview was cut mid-character) gets null and the
 * caller falls back to the top.
 */
export function previewOf(line: string): string {
  // The server's rule: strip leading `#`, trim, first 80 characters (not UTF-16 units).
  return Array.from(line.replace(/^#+/, "").trim()).slice(0, 80).join("");
}

export function blockStartLines(body: string, previews: string[]): Array<number | null> {
  const lines = body.split(/\r?\n/);
  let at = 0;
  return previews.map((p) => {
    if (!p) return null;
    for (let i = at; i < lines.length; i++) {
      if (previewOf(lines[i] ?? "") === p) {
        at = i + 1;
        return i + 1;
      }
    }
    return null;
  });
}

/**
 * The element to scroll to: the first block-level node at or after `line`, given the lines
 * the rendered nodes came from (ascending). Null when nothing starts that late.
 */
export function firstAtOrAfter(lines: number[], line: number): number | null {
  let best: number | null = null;
  for (const l of lines) if (l >= line && (best === null || l < best)) best = l;
  return best;
}
