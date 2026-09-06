import { useState } from "react";
import type { Citation as CitationT, Ring } from "@/api/client";
import { copyText } from "@/lib/clipboard";
import { useT } from "@/lib/i18n";
import { useToast } from "./Toast";

/**
 * The single most frequent action in the product: copy a citation to paste it back to an
 * agent. One click on the chip copies; the whole chip is the target, not a tiny icon.
 * A `<button>` so it is focusable and Enter/Space work.
 */
export function CitationChip({ citation, ring, size = "md", className = "" }: { citation: CitationT; ring?: Ring; size?: "sm" | "md"; className?: string }) {
  const toast = useToast();
  const t = useT();
  const [copied, setCopied] = useState(false);
  const r = ring ?? (Number(citation[1]) as Ring);
  const doCopy = async (e?: React.MouseEvent) => {
    e?.stopPropagation();
    const ok = await copyText(citation);
    if (ok) {
      setCopied(true);
      toast(t.citation.copied(citation));
      setTimeout(() => setCopied(false), 1200);
    } else toast(t.citation.clipboardUnavailable, "err");
  };
  return (
    <button
      type="button"
      onClick={doCopy}
      className={`group inline-flex items-center gap-1.5 rounded border font-mono tnum whitespace-nowrap cursor-copy transition-colors ${size === "sm" ? "h-5 px-1.5 text-2xs" : "h-6 px-2 text-xs"} ${className}`}
      style={{
        borderColor: copied ? "var(--ok)" : `color-mix(in oklch, var(--ring-${r}) 40%, var(--line))`,
        background: copied ? "var(--ok-bg)" : "var(--surface)",
      }}
      title={t.citation.copy}
      aria-label={t.citation.copyAria(citation)}
    >
      <span style={{ color: `var(--ring-${r})` }}>{citation.slice(0, 2)}</span>
      <span className="text-fg">{citation.slice(2)}</span>
      <span className={`text-2xs ${copied ? "text-ok" : "text-fg-faint group-hover:text-fg-muted"}`} aria-hidden>
        {copied ? "✓" : "⧉"}
      </span>
    </button>
  );
}
