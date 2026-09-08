import { lazy, Suspense, useEffect, useState } from "react";
import { api } from "@/api/client";
import { HubLine } from "@/components/HubLine";
import { ShortcutHelp } from "@/components/ShortcutHelp";
import { Kbd } from "@/components/ui";
import { LANG_NAME, setLang, useLang, useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { setMode, useMode } from "@/lib/mode";
import { href, navigate, SCREENS, SIMPLE_SCREENS, useRoute, type NavScreen } from "@/lib/router";
import { captureToken } from "@/lib/terminalToken";
import { useTheme } from "@/lib/theme";
import { AskScreen } from "@/screens/Ask";
import { ComplianceScreen } from "@/screens/Compliance";
import { ConsoleScreen } from "@/screens/Console";
import { GraphScreen } from "@/screens/Graph";
import { NoteScreen } from "@/screens/Note";
import { SearchScreen } from "@/screens/Search";
import { SimpleNotesScreen } from "@/screens/SimpleNotes";
import { StatusScreen } from "@/screens/Status";
import { TeamScreen } from "@/screens/Team";

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
  const mode = useMode();
  const simple = mode === "simple";
  const current = route.screen === "note" ? "notes" : route.screen;
  // Sticky: once the terminal screen has been opened it stays mounted, because it owns
  // processes rather than a view. See where it is rendered.
  const [visitedTerminals, setVisitedTerminals] = useState(false);
  useEffect(() => {
    if (route.screen === "terminals") setVisitedTerminals(true);
  }, [route.screen]);
  const other = lang === "en" ? "de" : "en";
  const nav = simple ? SIMPLE_SCREENS : SCREENS;

  useShortcuts(
    "global",
    [
      ...nav.map((s) => ({ keys: `g ${s.key}`, label: t.keys.goTo(t.nav[s.screen]), run: () => navigate(href(s.screen)) })),
      { keys: "Mod+k", label: t.keys.search, run: () => navigate(href("search")), inInputs: true },
      { keys: "Shift+T", label: t.keys.toggleTheme, run: theme.cycle },
      { keys: "Shift+L", label: t.keys.toggleLanguage, run: () => setLang(other) },
    ],
    [theme.choice, lang, mode],
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

  // In simple mode three screens are replaced; every other one is rendered exactly as it is
  // in the full view, because an address has to keep working — the launcher opens the page
  // with a terminal token in it, and `Technical details` links straight to Status.
  const simpleScreen = simple && (route.screen === "search" || route.screen === "notes" || route.screen === "note" || route.screen === "team");

  return (
    <div className="h-screen grid grid-cols-[13rem_1fr] grid-rows-[1fr] overflow-hidden">
      <aside className="border-r bg-surface flex flex-col min-h-0">
        <div className={`px-4 flex items-center gap-2 ${simple ? "h-14" : "h-12 border-b"}`}>
          <Logo />
          <span className="font-semibold tracking-tight">Cyberbrain</span>
        </div>
        <nav className={simple ? "p-2 flex-1 flex flex-col gap-0.5" : "py-2 flex-1"}>
          {simple ? (
            nav.map((s) => (
              <a key={s.screen} href={href(s.screen)} className={`flex items-center gap-2.5 px-2.5 py-2 rounded-lg text-sm ${current === s.screen ? "bg-surface-2 text-fg font-medium" : "text-fg-muted hover:text-fg"}`} aria-current={current === s.screen ? "page" : undefined}>
                <NavIcon screen={s.screen} />
                {s.screen === "search" ? t.ask.navLabel : t.nav[s.screen]}
              </a>
            ))
          ) : (
            <ul>
              {nav.map((s) => (
                <li key={s.screen}>
                  <a href={href(s.screen)} className={`flex items-center gap-2 px-4 py-1.5 text-sm ${current === s.screen ? "row-selected font-medium" : "text-fg-muted hover:text-fg"}`} aria-current={current === s.screen ? "page" : undefined}>
                    <span className="flex-1">{t.nav[s.screen]}</span>
                    <span className="text-2xs text-fg-faint font-mono">g {s.key}</span>
                  </a>
                </li>
              ))}
            </ul>
          )}
        </nav>
        <div className="px-4 py-3 border-t space-y-2 text-2xs text-fg-faint">
          {api.transport === "mock" ? (
            <div className="rounded border border-warn/50 bg-warn-bg text-warn px-2 py-1.5 leading-snug" role="status">
              <div className="font-medium">{t.app.mockTitle}</div>
              <div>{t.app.mockBody}</div>
            </div>
          ) : simple ? null : (
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
            {simple ? null : <Kbd keys="?" className="shrink-0" />}
          </div>
          {/* `truncate`, because this label is translated: a longer word in some language
              has to clip rather than run out of a 13rem sidebar, which is what the first
              wording of it did. */}
          <button className="btn btn-sm w-full justify-center truncate" onClick={() => setMode(simple ? "expert" : "simple")} title={t.mode.title}>
            {simple ? t.mode.toExpert : t.mode.toSimple}
          </button>
        </div>
      </aside>
      <main className="min-h-0 min-w-0 overflow-hidden">
        {simpleScreen ? (
          <>
            {route.screen === "search" ? <AskScreen route={route} /> : null}
            {route.screen === "notes" || route.screen === "note" ? <SimpleNotesScreen route={route} /> : null}
            {route.screen === "team" ? <TeamScreen /> : null}
          </>
        ) : (
          <>
            {route.screen === "search" ? <SearchScreen route={route} /> : null}
            {route.screen === "notes" || route.screen === "note" ? <NoteScreen route={route} /> : null}
            {route.screen === "team" ? <TeamScreen /> : null}
            {route.screen === "graph" ? <GraphScreen /> : null}
            {route.screen === "usage" ? <UsageScreen route={route} /> : null}
            {route.screen === "compliance" ? <ComplianceScreen route={route} /> : null}
            {route.screen === "status" ? <StatusScreen /> : null}
            {route.screen === "console" ? <ConsoleScreen /> : null}
          </>
        )}
        {/*
          Mounted once it has been visited, and kept mounted after that — `hidden` rather
          than unmounted.

          Every other screen is a view of the store and costs nothing to rebuild. This one
          owns running processes: unmounting it closes the sockets, and the server kills what
          was behind them. Clicking "Status" ended somebody's ssh session, with no warning
          and nothing to say afterwards. React has no way to keep a subtree alive across a
          route change other than not taking it down, so it is not taken down.
        */}
        {visitedTerminals ? (
          <Suspense fallback={<div className="p-4 text-2xs text-fg-faint">{t.common.loading}</div>}>
            <div hidden={route.screen !== "terminals"} className="h-full">
              <TerminalsScreen route={route} />
            </div>
          </Suspense>
        ) : null}
      </main>
      <ShortcutHelp />
    </div>
  );
}

/** Simple mode labels its three entries with a shape as well as a word; the full view does not. */
function NavIcon({ screen }: { screen: NavScreen }) {
  const common = { width: 16, height: 16, viewBox: "0 0 16 16", fill: "none", "aria-hidden": true } as const;
  if (screen === "search")
    return (
      <svg {...common} className="shrink-0 opacity-80">
        <circle cx="7" cy="7" r="4.6" stroke="currentColor" strokeWidth="1.5" />
        <path d="M10.4 10.4 14 14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      </svg>
    );
  if (screen === "notes")
    return (
      <svg {...common} className="shrink-0 opacity-80">
        <rect x="3" y="2" width="10" height="12" rx="2" stroke="currentColor" strokeWidth="1.4" />
        <path d="M5.8 5.6h4.4M5.8 8h4.4M5.8 10.4h2.6" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      </svg>
    );
  return (
    <svg {...common} className="shrink-0 opacity-80">
      <circle cx="6" cy="6" r="2.4" stroke="currentColor" strokeWidth="1.4" />
      <path d="M2.4 13c0-2.2 1.6-3.6 3.6-3.6S9.6 10.8 9.6 13" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      <path d="M10.6 4.2a2.2 2.2 0 0 1 0 4M11.4 9.7c1.4.4 2.3 1.7 2.3 3.3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
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
