/**
 * Keyboard shortcuts. One document-level listener; screens register bindings with a scope
 * and the most recently mounted scope wins. Two-key chords (`g s`) are supported.
 *
 * Bindings never fire while typing in an input unless declared `inInputs`, except Escape
 * which always propagates.
 */
import { useEffect } from "react";

export interface Binding {
  /** "/" | "Escape" | "g s" | "Shift+C" | "Mod+k" (Mod = Ctrl or ⌘) */
  keys: string;
  label: string;
  run: (e: KeyboardEvent) => void;
  inInputs?: boolean;
  /** Hidden from the help overlay. */
  hidden?: boolean;
}

interface Scope {
  name: string;
  bindings: Binding[];
}

const scopes: Scope[] = [];
const listeners = new Set<() => void>();
let pendingChord: string | null = null;
let chordTimer: ReturnType<typeof setTimeout> | null = null;

const isEditable = (el: EventTarget | null) => {
  if (!(el instanceof HTMLElement)) return false;
  return el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName);
};

function keyName(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push("Mod");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey && e.key.length > 1) parts.push("Shift");
  let k = e.key;
  if (k === " ") k = "Space";
  if (k.length === 1 && e.shiftKey && /[A-Z]/.test(k)) parts.push("Shift"), (k = k.toUpperCase());
  else if (k.length === 1) k = k.toLowerCase();
  parts.push(k);
  return parts.join("+");
}

function dispatch(e: KeyboardEvent) {
  if (e.defaultPrevented) return;
  const name = keyName(e);
  const editing = isEditable(e.target);
  const chord = pendingChord ? `${pendingChord} ${name}` : null;
  // Newest scope first.
  for (let i = scopes.length - 1; i >= 0; i--) {
    for (const b of scopes[i]?.bindings ?? []) {
      if (editing && !b.inInputs && name !== "Escape") continue;
      if (chord && b.keys === chord) {
        clearChord();
        e.preventDefault();
        b.run(e);
        return;
      }
      if (!pendingChord && b.keys === name) {
        e.preventDefault();
        b.run(e);
        return;
      }
    }
  }
  if (pendingChord) {
    clearChord();
    return;
  }
  // Start a chord if any binding begins with this key.
  if (!editing && scopes.some((s) => s.bindings.some((b) => b.keys.startsWith(name + " ")))) {
    pendingChord = name;
    chordTimer = setTimeout(clearChord, 900);
    e.preventDefault();
  }
}

function clearChord() {
  pendingChord = null;
  if (chordTimer) clearTimeout(chordTimer);
  chordTimer = null;
}

let installed = false;
function install() {
  if (installed) return;
  installed = true;
  document.addEventListener("keydown", dispatch);
}

export function useShortcuts(name: string, bindings: Binding[], deps: unknown[] = []) {
  useEffect(() => {
    install();
    const scope: Scope = { name, bindings };
    scopes.push(scope);
    listeners.forEach((l) => l());
    return () => {
      const i = scopes.indexOf(scope);
      if (i >= 0) scopes.splice(i, 1);
      listeners.forEach((l) => l());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

/** Snapshot of every active binding, for the help overlay. */
export function activeBindings(): Array<{ scope: string; binding: Binding }> {
  const out: Array<{ scope: string; binding: Binding }> = [];
  for (const s of scopes) for (const b of s.bindings) if (!b.hidden) out.push({ scope: s.name, binding: b });
  return out;
}

export function onBindingsChange(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Render "Mod+k" as the platform's symbol. */
export function prettyKeys(keys: string): string[] {
  const mac = /Mac|iPhone|iPad/.test(navigator.platform);
  return keys.split(" ").map((k) =>
    k
      .replace("Mod", mac ? "⌘" : "Ctrl")
      .replace("Shift+", mac ? "⇧" : "Shift+")
      .replace("Escape", "Esc")
      .replace("ArrowDown", "↓")
      .replace("ArrowUp", "↑")
      .replace("Enter", "↵"),
  );
}
