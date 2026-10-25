/**
 * Icons, drawn inline.
 *
 * No icon package: every one of these is a handful of strokes on a 20 px grid, which is
 * cheaper than a dependency whose licence has to be checked (`npm run licenses`) and whose
 * bundle would carry hundreds of glyphs nobody uses. They draw with `currentColor`, so a
 * state colour or a nav highlight recolours them without a second asset. Decorative by
 * default (`aria-hidden`); the word next to them carries the meaning.
 */
import type { SVGProps } from "react";

const PATHS = {
  status: "M4 16V9m4.5 7V4m4.5 12v-5m3.5 5V7",
  search: "M8.8 14.6a5.8 5.8 0 1 0 0-11.6 5.8 5.8 0 0 0 0 11.6Zm4.2-1.6 3.6 3.6",
  notes: "M6 2.8h5.6L15.5 6.7V16a1.2 1.2 0 0 1-1.2 1.2H6A1.2 1.2 0 0 1 4.8 16V4A1.2 1.2 0 0 1 6 2.8ZM11.4 3v3.8h3.9M7.5 10.4h5M7.5 13.2h5",
  team: "M7.5 9a2.6 2.6 0 1 0 0-5.2 2.6 2.6 0 0 0 0 5.2ZM2.8 16c0-2.6 2-4.4 4.7-4.4s4.7 1.8 4.7 4.4M13 4.2a2.4 2.4 0 0 1 0 4.6M14.2 11.8c1.7.4 3 1.8 3 4.2",
  review: "M10 2.8 16 5v4.6c0 3.4-2.4 5.9-6 7.2-3.6-1.3-6-3.8-6-7.2V5l6-2.2ZM7.2 9.8l2 2 3.7-3.9",
  graph: "M10 7.4a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM5 16.4a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM15 16.4a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM9 7.2 6 12.6M11 7.2l3 5.4",
  usage: "M3.5 16.5h13M5.8 16.5V10m4.2 6.5V4.5m4.2 12V8",
  compliance: "M10 2.8 16 5v4.6c0 3.4-2.4 5.9-6 7.2-3.6-1.3-6-3.8-6-7.2V5l6-2.2Z",
  console: "M3.5 5.2A1.7 1.7 0 0 1 5.2 3.5h9.6a1.7 1.7 0 0 1 1.7 1.7v9.6a1.7 1.7 0 0 1-1.7 1.7H5.2a1.7 1.7 0 0 1-1.7-1.7V5.2ZM6.8 8l2.4 2-2.4 2m4.4 0h2.6",
  terminals: "M3.5 4.9A1.4 1.4 0 0 1 4.9 3.5h10.2a1.4 1.4 0 0 1 1.4 1.4v7.2a1.4 1.4 0 0 1-1.4 1.4H4.9a1.4 1.4 0 0 1-1.4-1.4V4.9ZM7.5 16.5h5M10 13.5v3",
  database: "M10 8.2c3.3 0 6-1.1 6-2.5s-2.7-2.5-6-2.5-6 1.1-6 2.5 2.7 2.5 6 2.5ZM4 5.7v8.6c0 1.4 2.7 2.5 6 2.5s6-1.1 6-2.5V5.7M4 10c0 1.4 2.7 2.5 6 2.5s6-1.1 6-2.5",
  index: "M6 2.8h5.6L15.5 6.7V16a1.2 1.2 0 0 1-1.2 1.2H6A1.2 1.2 0 0 1 4.8 16V4A1.2 1.2 0 0 1 6 2.8ZM11.4 3v3.8h3.9M7.5 11.2l1.8 1.8 3.2-3.4",
  cpu: "M6.5 5.5h7a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1ZM8.5 8.5h3v3h-3ZM8 2.8v2.7M12 2.8v2.7M8 14.5v2.7M12 14.5v2.7M2.8 8h2.7M2.8 12h2.7M14.5 8h2.7M14.5 12h2.7",
  server: "M4.5 3.5h11a1 1 0 0 1 1 1v3.2a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1ZM4.5 10.3h11a1 1 0 0 1 1 1v3.2a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1v-3.2a1 1 0 0 1 1-1ZM6.5 6.1h.01M6.5 12.9h.01",
  bolt: "M11 2.8 4.8 11h4.2l-.8 6.2L15.2 9H11l0-6.2Z",
  cube: "M10 2.8 16.2 6.2v7.6L10 17.2 3.8 13.8V6.2L10 2.8ZM3.9 6.3 10 9.7l6.1-3.4M10 9.7v7.4",
  refresh: "M16 9a6 6 0 0 0-10.5-3.5L4 7M4 3.5V7h3.5M4 11a6 6 0 0 0 10.5 3.5L16 13M16 16.5V13h-3.5",
  check: "M4.5 10.5 8.4 14.4 15.5 6.2",
  alert: "M10 3.6 17.4 16.4H2.6L10 3.6ZM10 8.4v3.6M10 14.2h.01",
  cross: "M5.5 5.5l9 9m0-9-9 9",
  dash: "M5.5 10h9",
  info: "M10 9v4.6M10 6.4h.01M10 17a7 7 0 1 0 0-14 7 7 0 0 0 0 14Z",
  menu: "M3.5 5.5h13M3.5 10h13M3.5 14.5h13",
  close: "M5 5l10 10M15 5 5 15",
  chevron: "M5.5 7.8 10 12.2l4.5-4.4",
  folder: "M3.5 5.6A1.6 1.6 0 0 1 5.1 4h2.9l1.6 1.8h5.3a1.6 1.6 0 0 1 1.6 1.6v6.5a1.6 1.6 0 0 1-1.6 1.6H5.1a1.6 1.6 0 0 1-1.6-1.6V5.6Z",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 20, className, ...rest }: { name: IconName; size?: number } & Omit<SVGProps<SVGSVGElement>, "name">) {
  return (
    <svg width={size} height={size} viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden className={`shrink-0 ${className ?? ""}`} {...rest}>
      <path d={PATHS[name]} />
    </svg>
  );
}
