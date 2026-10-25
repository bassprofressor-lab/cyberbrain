import { lazy, Suspense, useEffect, useState } from "react";
import { api } from "@/api/client";
import { HubLine } from "@/components/HubLine";
import { Icon } from "@/components/icons";
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
import { ReviewScreen } from "@/screens/Review";
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

/** Contiguous runs of the sidebar order; `status` stands alone above the first heading. */
const NAV_GROUP: Partial<Record<NavScreen, "work" | "operations" | "tools">> = {
  search: "work",
  notes: "work",
  team: "work",
  review: "work",
  graph: "work",
  usage: "operations",
  compliance: "operations",
  console: "tools",
  terminals: "tools",
};

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
  // Below 768px the navigation is a sheet under the header; it closes when a page is chosen.
  const [menuOpen, setMenuOpen] = useState(false);
  useEffect(() => setMenuOpen(false), [route.screen]);
  useEffect(() => {
    if (!menuOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setMenuOpen(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menuOpen]);
  const other = lang === "en" ? "de" : "en";
  const nav = simple ? SIMPLE_SCREENS : SCREENS;
  // One pane of the launcher's side-by-side window: the command line alone, without the
  // sidebar. Four projects each carrying a full navigation left the lines 180 px wide.
  const bare = route.screen === "console" && route.query.get("pane") === "1";

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
    // Below 768px the sidebar is a header: brand, a nav strip that scrolls sideways, and
    // the same footer folded into one row. Nothing is dropped, it only changes direction —
    // a 13rem column next to a 390px screen left the content 180px wide.
    <div className={`h-dvh grid grid-cols-1 overflow-hidden ${bare ? "grid-rows-1" : "grid-rows-[auto_1fr] md:grid-rows-1 md:grid-cols-[16rem_minmax(0,1fr)]"}`}>
      <aside className="app-side min-w-0" style={bare ? { display: "none" } : undefined}>
        <div className="app-brand">
          <img src="./logo.png" alt="" width={32} height={32} />
          <span>{t.design.brand}</span>
          <button className="btn btn-icon btn-ghost app-menu-btn md:hidden" onClick={() => setMenuOpen((o) => !o)} aria-expanded={menuOpen} aria-controls="app-sheet" aria-label={menuOpen ? t.design.menuClose : t.design.menu} title={menuOpen ? t.design.menuClose : t.design.menu}>
            <Icon name={menuOpen ? "close" : "menu"} />
          </button>
        </div>
        <div id="app-sheet" className={`app-sheet scroll-thin ${menuOpen ? "is-open" : ""}`}>
        <nav className="app-nav" aria-label={t.design.navAria}>
          <ul className="nav-list">
            {nav.map((s, i) => {
              const group = simple ? null : NAV_GROUP[s.screen];
              const label = simple && s.screen === "search" ? t.ask.navLabel : t.nav[s.screen];
              return (
                <li key={s.screen}>
                  {group && group !== (NAV_GROUP[nav[i - 1]?.screen as NavScreen] ?? null) ? (
                    <div className="nav-group" aria-hidden>
                      {t.design.group[group]}
                    </div>
                  ) : null}
                  <a href={href(s.screen)} className={`nav-item ${current === s.screen ? "is-active" : ""}`} aria-current={current === s.screen ? "page" : undefined}>
                    <Icon name={s.screen} />
                    <span className="truncate">{label}</span>
                    {simple ? null : <span className="nav-key">g {s.key}</span>}
                  </a>
                </li>
              );
            })}
          </ul>
        </nav>
        <div className="app-foot">
          {api.transport === "mock" ? (
            <div className="mock-notice basis-full md:basis-auto" role="status">
              <div className="font-semibold">{t.app.mockTitle}</div>
              <div>{t.app.mockBody}</div>
            </div>
          ) : simple ? null : (
            <div>{t.app.connected("/api/v1")}</div>
          )}
          <HubLine />
          <div className="app-foot-controls md:flex-col md:items-stretch">
            <div className="flex items-center gap-1.5">
              <button className="btn btn-sm min-w-0 md:flex-1 justify-start truncate" onClick={theme.cycle} title={t.app.themeTitle}>
                {theme.choice === "system" ? t.app.themeSystem(theme.effective === "dark" ? t.app.themeDark : t.app.themeLight) : theme.choice === "dark" ? t.app.themeDark : t.app.themeLight}
              </button>
              <button className="btn btn-sm shrink-0 font-medium tracking-wide" onClick={() => setLang(other)} title={`${t.app.languageTitle} · ${LANG_NAME[other]}`} aria-label={t.keys.toggleLanguage} lang={other}>
                {other.toUpperCase()}
              </button>
              {simple ? null : <Kbd keys="?" className="shrink-0" />}
            </div>
            {/* `truncate`, because this label is translated: a longer word in some language
                has to clip rather than run out of a 14rem sidebar, which is what the first
                wording of it did. */}
            <button className="btn btn-sm justify-center truncate" onClick={() => setMode(simple ? "expert" : "simple")} title={t.mode.title}>
              {simple ? t.mode.toExpert : t.mode.toSimple}
            </button>
          </div>
        </div>
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
            {route.screen === "review" ? <ReviewScreen /> : null}
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
