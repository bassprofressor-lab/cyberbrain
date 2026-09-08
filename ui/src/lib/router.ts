/**
 * Hash router. The page is served from inside a binary; a hash route needs no server-side
 * fallback and survives `file://` if someone opens dist/index.html directly.
 *
 *   #/search?q=…&ring=2   #/notes?ring=3   #/note/<name|id>   #/graph   #/usage?days=30
 *   #/compliance#audit
 *   #/status   #/console
 */
import { useEffect, useState } from "react";

export type Screen = "search" | "notes" | "note" | "graph" | "usage" | "compliance" | "status" | "console";

export interface Route {
  screen: Screen;
  /** Path segment after the screen, e.g. the note name. */
  param: string | null;
  query: URLSearchParams;
  /** In-screen anchor, e.g. compliance section. */
  anchor: string | null;
}

/** Every screen with an entry in the sidebar. `note` is reached from `notes`, not from the nav. */
export type NavScreen = Exclude<Screen, "note">;

export const SCREENS: Array<{ screen: NavScreen; key: string }> = [
  { screen: "status", key: "t" },
  { screen: "search", key: "s" },
  { screen: "notes", key: "n" },
  { screen: "graph", key: "g" },
  { screen: "usage", key: "u" },
  { screen: "compliance", key: "c" },
  // Last, and not because it matters least: it is the one screen that can change the store
  // in ways the others cannot, so it is not the thing a hand lands on.
  { screen: "console", key: "k" },
];

export function parseRoute(hash: string): Route {
  const raw = hash.replace(/^#\/?/, "");
  const [pathAndQuery, anchor = null] = raw.split("#") as [string, string | undefined];
  const [path = "", query = ""] = pathAndQuery.split("?") as [string, string | undefined];
  // Status is the landing screen: it answers "is this store healthy and what did it cost"
  // before the reader has typed anything. Search is one keystroke away (Mod+K).
  const [seg = "status", ...rest] = path.split("/");
  const screen = (["search", "notes", "note", "graph", "usage", "compliance", "status", "console"] as Screen[]).includes(seg as Screen) ? (seg as Screen) : "status";
  const param = rest.length ? decodeURIComponent(rest.join("/")) : null;
  return { screen, param, query: new URLSearchParams(query), anchor };
}

export function href(screen: Screen, param?: string | null, query?: Record<string, string | number | undefined | null>, anchor?: string): string {
  let h = `#/${screen}`;
  if (param) h += `/${encodeURIComponent(param)}`;
  if (query) {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) if (v !== undefined && v !== null && v !== "") p.set(k, String(v));
    const s = p.toString();
    if (s) h += `?${s}`;
  }
  if (anchor) h += `#${anchor}`;
  return h;
}

export function navigate(to: string, replace = false) {
  if (replace) history.replaceState(null, "", to);
  else location.hash = to.startsWith("#") ? to.slice(1) : to;
  if (replace) window.dispatchEvent(new HashChangeEvent("hashchange"));
}

export function useRoute(): Route {
  const [route, setRoute] = useState(() => parseRoute(location.hash));
  useEffect(() => {
    const on = () => setRoute(parseRoute(location.hash));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  return route;
}
