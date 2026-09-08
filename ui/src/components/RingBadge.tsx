import type { Ring } from "@/api/client";
import { useT } from "@/lib/i18n";

/**
 * Ring identity, encoded three ways so it survives both themes and colour vision:
 * hue, fill (solid → hollow → dashed) and the literal label.
 */
export function RingGlyph({ ring, size = 10, className }: { ring: Ring; size?: number; className?: string }) {
  const c = `var(--ring-${ring})`;
  const s = size;
  const h = s / 2;
  let shape: React.ReactNode;
  switch (ring) {
    case 0:
      shape = <path d={`M${h} 0.5 L${s - 0.5} ${h} L${h} ${s - 0.5} L0.5 ${h} Z`} fill={c} />;
      break;
    case 1:
      shape = <rect x="1" y="1" width={s - 2} height={s - 2} rx="1" fill={c} />;
      break;
    case 2:
      shape = <circle cx={h} cy={h} r={h - 0.75} fill={c} />;
      break;
    case 3:
      shape = <circle cx={h} cy={h} r={h - 1.25} fill="none" stroke={c} strokeWidth="1.6" />;
      break;
    default:
      shape = <circle cx={h} cy={h} r={h - 1.25} fill="none" stroke={c} strokeWidth="1.4" strokeDasharray="2 1.6" />;
  }
  return (
    <svg width={s} height={s} viewBox={`0 0 ${s} ${s}`} className={className} aria-hidden>
      {shape}
    </svg>
  );
}

export function RingBadge({ ring, showName = false, className = "" }: { ring: Ring; showName?: boolean; className?: string }) {
  const t = useT();
  return (
    <span
      className={`inline-flex items-center gap-1.5 h-5 px-1.5 rounded border font-mono text-2xs font-medium tnum whitespace-nowrap ${className}`}
      style={{ color: `var(--ring-${ring})`, background: `var(--ring-${ring}-bg)`, borderColor: `color-mix(in oklch, var(--ring-${ring}) 35%, transparent)` }}
      title={`Ring ${ring} — ${t.rings.label[ring]}: ${t.rings.help[ring]}`}
    >
      <RingGlyph ring={ring} size={9} />
      r{ring}
      {showName ? <span className="font-sans font-normal opacity-90">{t.rings.label[ring]}</span> : null}
    </span>
  );
}

export function RingLegend({ className = "" }: { className?: string }) {
  const t = useT();
  return (
    <div className={`flex flex-wrap gap-x-3 gap-y-1 ${className}`}>
      {([0, 1, 2, 3, 4] as Ring[]).map((r) => (
        <span key={r} className="inline-flex items-center gap-1.5 text-xs text-fg-muted" title={t.rings.help[r]}>
          <RingGlyph ring={r} size={10} />
          <span className="font-mono tnum" style={{ color: `var(--ring-${r})` }}>
            r{r}
          </span>
          {t.rings.label[r]}
        </span>
      ))}
    </div>
  );
}

/**
 * The same tier for simple mode: the plain-language name, no `r0`, no glyph legend to learn.
 *
 * Hue and label still carry it together — a chip that is only coloured says nothing to a
 * reader who cannot see the difference, which is why the name is never dropped in favour of
 * the colour alone.
 */
export function RingChip({ ring, className = "" }: { ring: Ring; className?: string }) {
  const t = useT();
  return (
    <span
      // `self-start`, because every place this is used is a column flex: without it the chip
      // stretches to the width of the card and reads as a banner.
      className={`inline-flex self-start items-center gap-1.5 h-6 pl-2 pr-2.5 rounded-full border text-2xs font-medium whitespace-nowrap ${className}`}
      style={{ color: `var(--ring-${ring})`, background: `var(--ring-${ring}-bg)`, borderColor: `color-mix(in oklch, var(--ring-${ring}) 45%, transparent)` }}
    >
      <RingGlyph ring={ring} size={8} />
      {t.rings.plain[ring]}
    </span>
  );
}
