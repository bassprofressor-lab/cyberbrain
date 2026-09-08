/**
 * Which of the two densities the page is in.
 *
 * Same shape as the language: module state, one localStorage key, no provider — the router
 * needs to read it outside React (it decides the landing screen) and the sidebar needs to
 * re-render when it changes.
 *
 * `simple` is the default, and that is the whole point rather than a taste: the page is
 * installed on a colleague's machine as often as on an operator's, and a first screen that
 * opens with `scan`, `doctor` and an embedding profile has already lost that reader. An
 * operator flips the switch once and it is remembered.
 *
 * Simple is a smaller *front*, not a smaller product. It changes the sidebar and the three
 * screens behind it; every other screen stays reachable by address, unchanged — including
 * the terminal, which the launcher opens by URL with its token.
 */
import { useSyncExternalStore } from "react";

export type Mode = "simple" | "expert";

const KEY = "cyberbrain.mode";
const listeners = new Set<() => void>();

function read(): Mode {
  try {
    return localStorage.getItem(KEY) === "expert" ? "expert" : "simple";
  } catch {
    /* storage may be unavailable; simple is the default either way */
    return "simple";
  }
}

let current: Mode = read();

export function getMode(): Mode {
  return current;
}

export function setMode(mode: Mode) {
  if (mode === current) return;
  current = mode;
  try {
    localStorage.setItem(KEY, mode);
  } catch {
    /* the choice still applies to this page */
  }
  for (const l of listeners) l();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function useMode(): Mode {
  return useSyncExternalStore(subscribe, getMode, getMode);
}
