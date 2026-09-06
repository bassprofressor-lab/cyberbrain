import { useEffect, useState } from "react";
import { useT } from "@/lib/i18n";
import { activeBindings, onBindingsChange, useShortcuts } from "@/lib/keys";
import { Kbd } from "./ui";

export function ShortcutHelp() {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [, bump] = useState(0);
  useEffect(() => onBindingsChange(() => bump((n) => n + 1)), []);
  useShortcuts("help", [
    { keys: "?", label: t.keys.showShortcuts, run: () => setOpen((o) => !o), hidden: true },
    { keys: "Escape", label: t.keys.close, run: () => setOpen(false), inInputs: true, hidden: true },
  ]);
  if (!open) return null;
  const groups = new Map<string, ReturnType<typeof activeBindings>>();
  for (const b of activeBindings()) {
    if (b.scope === "help") continue;
    const g = groups.get(b.scope) ?? [];
    g.push(b);
    groups.set(b.scope, g);
  }
  return (
    <div className="fixed inset-0 z-40 bg-bg/70 flex items-start justify-center pt-[12vh]" onClick={() => setOpen(false)} role="dialog" aria-label={t.keys.dialogLabel}>
      <div className="panel shadow-panel w-[min(40rem,92vw)] max-h-[76vh] overflow-auto scroll-thin" onClick={(e) => e.stopPropagation()}>
        <header className="flex items-center justify-between px-4 h-10 border-b">
          <h2 className="text-sm font-semibold">{t.keys.title}</h2>
          <span className="text-xs text-fg-faint">
            <Kbd keys="?" /> {t.keys.hint.toggles} · <Kbd keys="Escape" /> {t.keys.hint.closes}
          </span>
        </header>
        <div className="p-4 grid gap-4 sm:grid-cols-2">
          {[...groups.entries()].map(([scope, bs]) => (
            <div key={scope}>
              <div className="label mb-1.5">{t.scopes[scope as keyof typeof t.scopes] ?? scope}</div>
              <ul className="space-y-1">
                {bs.map(({ binding }, i) => (
                  <li key={i} className="flex items-center justify-between gap-3 text-sm">
                    <span className="text-fg-muted">{binding.label}</span>
                    <Kbd keys={binding.keys} />
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
