import { useEffect } from "react";
import type { NoteDetail } from "@/api/client";
import { blockStartLines, firstAtOrAfter } from "@/lib/blocks";

/**
 * Scroll a note to the block a hit or a citation named, and flash it for a moment.
 *
 * The line a block starts at comes from matching the server's one-line previews against the
 * body; the element is the first rendered block at or after that line. Where either step
 * cannot be done the page stays where it is, at the top, which is what it did for every block
 * before.
 */
export function useBlockJump(d: NoteDetail | null, block: string | null, enabled = true) {
  useEffect(() => {
    if (!d || block === null || !enabled) return;
    const idx = Number(block);
    const pos = d.blocks.findIndex((b) => b.idx === idx);
    if (pos < 0) return;
    const line = blockStartLines(d.body, d.blocks.map((b) => b.preview))[pos];
    if (line === null || line === undefined) return;
    const els = [...document.querySelectorAll<HTMLElement>(".prose-note [data-line]")];
    const best = firstAtOrAfter(els.map((e) => Number(e.dataset.line)), line);
    const target = best === null ? undefined : els.find((e) => Number(e.dataset.line) === best);
    if (!target) return;
    target.scrollIntoView({ block: "start" });
    target.classList.add("block-flash");
    const timer = setTimeout(() => target.classList.remove("block-flash"), 2200);
    return () => clearTimeout(timer);
  }, [d, block, enabled]);
}
