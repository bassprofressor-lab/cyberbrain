/**
 * Hash router. The page is served from inside a binary; a hash route needs no server-side
 * fallback and survives `file://` if someone opens dist/index.html directly.
 *
 *   #/search?q=…&ring=2   #/notes?ring=3   #/note/<name|id>   #/graph   #/usage?days=30
 *   #/compliance#audit
 *   #/status   #/console   #/terminals?t=<token>   #/team
 */
import { useEffect, useState } from "react";
import { getMode } from "./mode";

export type Screen = "search" | "notes" | "note" | "graph" | "usage" | "compliance" | "status" | "console" | "terminals" | "team";

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

/**
 * The sidebar in expert mode.
 *
 * `team` was left out until 2026-09-10 for a reason that has since expired: back then it was
 * the notes list sorted by date, which this mode already has a screen for. Since a note can
 * carry a `bereich` it answers something no other screen does — what of this store's work
 * can leave the machine, and what stays — so it belongs in both views.
 */
export const SCREENS: Array<{ screen: NavScreen; key: string }> = [
  { screen: "status", key: "t" },
  { screen: "search", key: "s" },
  { screen: "notes", key: "n" },
  { screen: "team", key: "m" },
  { screen: "graph", key: "g" },
  { screen: "usage", key: "u" },
  { screen: "compliance", key: "c" },
  // Last, and not because it matters least: it is the one screen that can change the store
  // in ways the others cannot, so it is not the thing a hand lands on.
  { screen: "console", key: "k" },
  // Only useful when `serve --terminal` is on and the address carries its token; the screen
  // says so itself rather than the entry vanishing, because an entry that comes and goes is
  // one people stop looking for.
  { screen: "terminals", key: "e" },
];

/**
 * The sidebar in simple mode. Three entries, and the order is the order of the day: a
 * question first, what is written down second, what the others wrote third.
 */
export const SIMPLE_SCREENS: Array<{ screen: NavScreen; key: string }> = [
  { screen: "search", key: "s" },
  { screen: "notes", key: "n" },
  { screen: "team", key: "m" },
];

/**
 * Where an empty or unknown address lands. Expert mode answers "is this store healthy";
 * simple mode has no use for that question and opens on the one it does have.
 */
export function landingScreen(): Screen {
  return getMode() === "simple" ? "search" : "status";
}

export function parseRoute(hash: string, fallback: Screen = "status"): Route {
  const raw = hash.replace(/^#\/?/, "");
  const [pathAndQuery, anchor = null] = raw.split("#") as [string, string | undefined];
  const [path = "", query = ""] = pathAndQuery.split("?") as [string, string | undefined];
  // The landing screen depends on the mode, so it is passed in rather than decided here:
  // expert mode answers "is this store healthy and what did it cost" before the reader has
  // typed anything, and simple mode opens on the question box.
  const [seg = "", ...rest] = path.split("/");
  const screen = (["search", "notes", "note", "graph", "usage", "compliance", "status", "console", "terminals", "team"] as Screen[]).includes(seg as Screen) ? (seg as Screen) : fallback;
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
  const [route, setRoute] = useState(() => parseRoute(location.hash, landingScreen()));
  useEffect(() => {
    const on = () => setRoute(parseRoute(location.hash, landingScreen()));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  return route;
}
