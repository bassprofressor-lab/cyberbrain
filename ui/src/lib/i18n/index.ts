/**
 * Language, in the same shape as the theme: module state, a localStorage key, no provider.
 *
 * The dictionary is a nested object rather than dotted key strings, so `t.status.title` is
 * checked by the compiler, params are ordinary typed function arguments, and a key missing
 * from a translation is a build error (`de` is declared as `Dict`), never a blank in the UI.
 *
 * Nothing here talks to the network. A language file is compiled into the binary like the
 * rest of the page (SPEC §12.1): no fetch, no CDN, no font call.
 */
import { useSyncExternalStore } from "react";
import { de } from "./de";
import { en } from "./en";

export type Lang = "en" | "de";
export type Dict = typeof en;

const DICTS: Record<Lang, Dict> = { en, de };
export const LOCALE: Record<Lang, string> = { en: "en-GB", de: "de-DE" };
export const LANG_NAME: Record<Lang, string> = { en: "English", de: "Deutsch" };
export const LANGS: Lang[] = ["en", "de"];

const KEY = "cyberbrain.lang";
const listeners = new Set<() => void>();

function detect(): Lang {
  try {
    const stored = localStorage.getItem(KEY);
    if (stored === "en" || stored === "de") return stored;
  } catch {
    /* storage may be unavailable; fall through to the browser's own preference */
  }
  for (const tag of navigator.languages ?? [navigator.language]) {
    if (tag?.toLowerCase().startsWith("de")) return "de";
    if (tag?.toLowerCase().startsWith("en")) return "en";
  }
  return "en";
}

let current: Lang = detect();
document.documentElement.lang = current;

export function getLang(): Lang {
  return current;
}

/** The dictionary outside React: canvas labels, formatters, anything not in a component. */
export function dict(): Dict {
  return DICTS[current];
}

export function setLang(lang: Lang) {
  if (lang === current) return;
  current = lang;
  try {
    localStorage.setItem(KEY, lang);
  } catch {
    /* the choice still applies to this page */
  }
  document.documentElement.lang = lang;
  for (const l of listeners) l();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function useLang(): Lang {
  return useSyncExternalStore(subscribe, getLang, getLang);
}

/** The dictionary for the current language; re-renders the component when it changes. */
export function useT(): Dict {
  return DICTS[useLang()];
}
