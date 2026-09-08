import { lazy, Suspense, useEffect } from "react";
import { api } from "@/api/client";
import { HubLine } from "@/components/HubLine";
import { ShortcutHelp } from "@/components/ShortcutHelp";
import { Kbd } from "@/components/ui";
import { LANG_NAME, setLang, useLang, useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { href, navigate, SCREENS, useRoute } from "@/lib/router";
import { captureToken } from "@/lib/terminalToken";
import { useTheme } from "@/lib/theme";
import { ComplianceScreen } from "@/screens/Compliance";
import { ConsoleScreen } from "@/screens/Console";
import { GraphScreen } from "@/screens/Graph";
import { NoteScreen } from "@/screens/Note";
import { SearchScreen } from "@/screens/Search";
import { StatusScreen } from "@/screens/Status";

import { UsageScreen } from "@/screens/Usage";

/**
 * Loaded when somebody opens it, not before.
 *
 * xterm.js is around 300 KB, and most sessions never open a terminal. Everything else here
 * is in the one bundle on purpose — this is the exception, and it earns it.
 */
const TerminalsScreen = lazy(() =>
  import("@/screens/Terminals").then((m) => ({ default: m.TerminalsScreen })),
);

export function App() {
  const route = useRoute();
  const theme = useTheme();
  const t = useT();
  const lang = useLang();
  const current = route.screen === "note" ? "notes" : route.screen;
  const other = lang === "en" ? "de" : "en";

  useShortcuts(
    "global",
    [
      ...SCREENS.map((s) => ({ keys: `g ${s.key}`, label: t.keys.goTo(t.nav[s.screen]), run: () => navigate(href(s.screen)) })),
      { keys: "Mod+k", label: t.keys.search, run: () => navigate(href("search")), inInputs: true },
      { keys: "Shift+T", label: t.keys.toggleTheme, run: theme.cycle },
      { keys: "Shift+L", label: t.keys.toggleLanguage, run: () => setLang(other) },
    ],
    [theme.choice, lang],
  );

  // Before anything renders: take the terminal token out of the address and put the address
  // back the way it should look. Done here rather than in the terminal screen so that the
  // launcher can open any page with it and the token survives navigating away and back.
  useEffect(() => {
    if (captureToken(route.query)) {
      const rest = new URLSearchParams(route.query);
      rest.delete("t");
      navigate(href(route.screen, route.param, Object.fromEntries(rest)), true);
    }
  }, [route]);

  useEffect(() => {
    document.title = `${t.nav[current]}${route.param ? ` · ${route.param}` : ""} — Cyberbrain`;
  }, [current, route.param, t]);

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
                  <span className="flex-1">{t.nav[s.screen]}</span>
                  <span className="text-2xs text-fg-faint font-mono">g {s.key}</span>
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <div className="px-4 py-3 border-t space-y-2 text-2xs text-fg-faint">
          {api.transport === "mock" ? (
            <div className="rounded border border-warn/50 bg-warn-bg text-warn px-2 py-1.5 leading-snug" role="status">
              <div className="font-medium">{t.app.mockTitle}</div>
              <div>{t.app.mockBody}</div>
            </div>
          ) : (
            <div>{t.app.connected("/api/v1")}</div>
          )}
          <HubLine />
          <div className="flex items-center gap-1.5">
            <button className="btn btn-sm min-w-0 flex-1 justify-start truncate" onClick={theme.cycle} title={t.app.themeTitle}>
              {theme.choice === "system" ? t.app.themeSystem(theme.effective === "dark" ? t.app.themeDark : t.app.themeLight) : theme.choice === "dark" ? t.app.themeDark : t.app.themeLight}
            </button>
            <button className="btn btn-sm shrink-0 font-medium tracking-wide" onClick={() => setLang(other)} title={`${t.app.languageTitle} · ${LANG_NAME[other]}`} aria-label={t.keys.toggleLanguage} lang={other}>
              {other.toUpperCase()}
            </button>
            <Kbd keys="?" className="shrink-0" />
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
        {route.screen === "console" ? <ConsoleScreen /> : null}
        {route.screen === "terminals" ? (
          <Suspense fallback={<div className="p-4 text-2xs text-fg-faint">{t.common.loading}</div>}>
            <TerminalsScreen route={route} />
          </Suspense>
        ) : null}
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
