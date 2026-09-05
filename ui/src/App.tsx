import { useEffect } from "react";
import { api } from "@/api/client";
import { ShortcutHelp } from "@/components/ShortcutHelp";
import { Kbd } from "@/components/ui";
import { useShortcuts } from "@/lib/keys";
import { href, navigate, SCREENS, useRoute } from "@/lib/router";
import { useTheme } from "@/lib/theme";
import { ComplianceScreen } from "@/screens/Compliance";
import { GraphScreen } from "@/screens/Graph";
import { NoteScreen } from "@/screens/Note";
import { SearchScreen } from "@/screens/Search";
import { StatusScreen } from "@/screens/Status";
import { UsageScreen } from "@/screens/Usage";

export function App() {
  const route = useRoute();
  const theme = useTheme();
  const current = route.screen === "note" ? "notes" : route.screen;

  useShortcuts(
    "global",
    [
      ...SCREENS.map((s) => ({ keys: `g ${s.key}`, label: `Go to ${s.label}`, run: () => navigate(href(s.screen)) })),
      { keys: "Mod+k", label: "Search", run: () => navigate(href("search")), inInputs: true },
      { keys: "Shift+T", label: "Toggle theme", run: theme.cycle },
    ],
    [theme.choice],
  );

  useEffect(() => {
    document.title = `${SCREENS.find((s) => s.screen === current)?.label ?? "Cyberbrain"}${route.param ? ` · ${route.param}` : ""} — Cyberbrain`;
  }, [current, route.param]);

  return (
    <div className="h-screen grid grid-cols-[13rem_1fr] grid-rows-[1fr] overflow-hidden">
      <aside className="border-r bg-surface flex flex-col min-h-0">
        <div className="px-4 h-12 flex items-center gap-2 border-b">
          <Logo />
          <span className="font-semibold tracking-tight">Cyberbrain</span>
        </div>
        <nav className="py-2 flex-1">
          <ul>
            {SCREENS.map((s) => (
              <li key={s.screen}>
                <a href={href(s.screen)} className={`flex items-center gap-2 px-4 py-1.5 text-sm ${current === s.screen ? "row-selected font-medium" : "text-fg-muted hover:text-fg"}`} aria-current={current === s.screen ? "page" : undefined}>
                  <span className="flex-1">{s.label}</span>
                  <span className="text-2xs text-fg-faint font-mono">g {s.key}</span>
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <div className="px-4 py-3 border-t space-y-2 text-2xs text-fg-faint">
          {api.transport === "mock" ? (
            <div className="rounded border border-warn/50 bg-warn-bg text-warn px-2 py-1.5 leading-snug" role="status">
              <div className="font-medium">MOCK DATA</div>
              <div>No backend. Every number on every screen is fabricated for layout; the compliance statement is not evidence of anything.</div>
            </div>
          ) : (
            <div>
              connected to same-origin <code>/api/v1</code>
            </div>
          )}
          <div className="flex items-center justify-between">
            <button className="btn btn-sm" onClick={theme.cycle} title="Theme: system → light → dark">
              theme: {theme.choice === "system" ? `system (${theme.effective})` : theme.choice}
            </button>
            <Kbd keys="?" />
          </div>
        </div>
      </aside>
      <main className="min-h-0 min-w-0 overflow-hidden">
        {route.screen === "search" ? <SearchScreen route={route} /> : null}
        {route.screen === "notes" || route.screen === "note" ? <NoteScreen route={route} /> : null}
        {route.screen === "graph" ? <GraphScreen /> : null}
        {route.screen === "usage" ? <UsageScreen route={route} /> : null}
        {route.screen === "compliance" ? <ComplianceScreen route={route} /> : null}
        {route.screen === "status" ? <StatusScreen /> : null}
      </main>
      <ShortcutHelp />
    </div>
  );
}

function Logo() {
  return (
    <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden>
      <circle cx="9" cy="9" r="8" fill="none" stroke="var(--ring-4)" strokeWidth="1.2" strokeDasharray="2 1.5" />
      <circle cx="9" cy="9" r="5.6" fill="none" stroke="var(--ring-2)" strokeWidth="1.4" />
      <circle cx="9" cy="9" r="3" fill="var(--ring-0)" />
    </svg>
  );
}
