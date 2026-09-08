/**
 * A terminal in the page.
 *
 * xterm.js draws it and a WebSocket carries the bytes; the pseudo-console and the process
 * are the server's. Nothing here interprets what comes back — the escape sequences a
 * program emits are the terminal's business, and a half-emulator that handles colour but
 * not the alternate screen is worse than none.
 *
 * The token comes from the URL fragment (`#/terminals?t=…`), which a browser keeps to
 * itself and never sends to a server. It is not stored: reload the page from the address
 * the launcher opened and it is there again; arrive any other way and there is no terminal,
 * which is the intended answer.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { Empty, Section } from "@/components/ui";
import { useT } from "@/lib/i18n";
import type { Route } from "@/lib/router";
import { terminalToken } from "@/lib/terminalToken";

/** The page's own colours, so a terminal in it does not look like a hole in the window. */
function theme() {
  const css = getComputedStyle(document.documentElement);
  const pick = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  return {
    background: pick("--bg", "#0c0e12"),
    foreground: pick("--fg", "#e6e8ee"),
    cursor: pick("--fg", "#e6e8ee"),
  };
}

interface Saved {
  name: string;
  command: string;
}

/**
 * The saved list, behind the same token as the terminal.
 *
 * A header rather than a query parameter: a token in a request line reaches every log that
 * records one, and not being in one is the whole point of this token.
 */
async function profiles(token: string, next?: Saved[]): Promise<{ path: string | null; profiles: Saved[] }> {
  const res = await fetch("/api/v1/terminal/profiles", {
    method: next ? "PUT" : "GET",
    headers: {
      "x-cyberbrain-terminal-token": token,
      ...(next ? { "Content-Type": "application/json" } : {}),
    },
    ...(next ? { body: JSON.stringify({ profile: next }) } : {}),
    credentials: "omit",
    cache: "no-store",
  });
  if (!res.ok) {
    // The server answers refusals in the shape every other route uses. Showing the raw body
    // put `{"error":{"code":…}}` into a paragraph of prose; the message inside it is the
    // sentence somebody wrote to be read.
    const body = await res.text();
    let message = body;
    try {
      message = JSON.parse(body)?.error?.message ?? body;
    } catch {
      /* not the usual shape; the body is all there is */
    }
    throw new Error(message);
  }
  return res.json();
}

interface Props {
  route: Route;
}

export function TerminalsScreen({ route }: Props) {
  const t = useT();
  // The address may still carry it on a first load; after that it lives in memory.
  const token = route.query.get("t") ?? terminalToken();
  const [command, setCommand] = useState("");
  /** Each pane is a session; the key is what makes React build a fresh one. */
  const [panes, setPanes] = useState<Array<{ id: number; command: string }>>([]);
  const next = useRef(0);
  const [saved, setSaved] = useState<Saved[]>([]);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const [savedWhy, setSavedWhy] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    profiles(token)
      .then((r) => {
        setSaved(r.profiles);
        setSavedPath(r.path);
      })
      // A list that will not load is worth saying out loud: it usually means the file has a
      // typo in it, and silently showing none is how somebody concludes it was lost.
      .catch((e) => setSavedWhy(String(e.message ?? e)));
  }, [token]);

  const write = useCallback(
    async (list: Saved[]) => {
      if (!token) return;
      try {
        const r = await profiles(token, list);
        setSaved(r.profiles);
        setSavedPath(r.path);
        setSavedWhy(null);
      } catch (e) {
        setSavedWhy(String((e as Error).message ?? e));
      }
    },
    [token],
  );

  const start = useCallback(
    (line: string) => {
      setPanes((p) => [...p, { id: next.current++, command: line }]);
      setCommand("");
    },
    [],
  );

  if (!token) {
    return (
      <Section title={t.terminals.title}>
        <Empty title={t.terminals.noToken}>{t.terminals.noTokenHow}</Empty>
      </Section>
    );
  }

  return (
    <div className="h-full flex flex-col min-h-0">
      <Section title={t.terminals.title} className="flex-1 min-h-0 flex flex-col">
        <p className="text-2xs text-fg-faint mb-3 max-w-prose">{t.terminals.lead}</p>

        <form
          className="flex items-center gap-2 mb-3"
          onSubmit={(e) => {
            e.preventDefault();
            start(command);
          }}
        >
          <input
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            aria-label={t.terminals.commandAria}
            placeholder={t.terminals.placeholder}
            spellCheck={false}
            autoComplete="off"
            className="flex-1 bg-transparent font-mono text-xs rounded-sm border-b py-1 placeholder:text-fg-faint"
          />
          <button type="submit" className="text-2xs text-fg-muted hover:text-fg">
            {t.terminals.open}
          </button>
          <button
            type="button"
            className="text-2xs text-fg-muted hover:text-fg"
            onClick={() => start("")}
          >
            {t.terminals.openShell}
          </button>
          <button
            type="button"
            className="text-2xs text-fg-muted hover:text-fg disabled:opacity-40"
            disabled={command.trim() === ""}
            onClick={() => {
              const name = window.prompt(t.terminals.namePrompt, defaultName(command));
              if (!name) return;
              void write([...saved.filter((x) => x.name !== name), { name, command }]);
            }}
          >
            {t.terminals.save}
          </button>
        </form>

        {saved.length > 0 || savedWhy ? (
          <div className="mb-3">
            <div className="flex flex-wrap items-center gap-1.5">
              <span className="text-2xs text-fg-faint">{t.terminals.saved}</span>
              {saved.map((x) => (
                <span key={x.name} className="inline-flex items-center gap-1 border rounded px-1.5 py-0.5">
                  <button
                    type="button"
                    className="text-2xs font-mono hover:text-fg"
                    title={x.command}
                    onClick={() => start(x.command)}
                  >
                    {x.name}
                  </button>
                  <button
                    type="button"
                    aria-label={t.terminals.forget(x.name)}
                    className="text-2xs text-fg-faint hover:text-danger"
                    onClick={() => void write(saved.filter((y) => y.name !== x.name))}
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
            {savedWhy ? (
              <p role="alert" className="text-2xs text-danger mt-1">
                {savedWhy}
              </p>
            ) : null}
            {savedPath ? <p className="text-2xs text-fg-faint mt-1">{t.terminals.savedIn(savedPath)}</p> : null}
          </div>
        ) : null}

        <div className="flex-1 min-h-0 overflow-y-auto space-y-3">
          {panes.length === 0 ? (
            <Empty title={t.terminals.empty}>{t.terminals.emptyHow}</Empty>
          ) : null}
          {panes.map((p) => (
            <Pane
              key={p.id}
              token={token}
              command={p.command}
              onClose={() => setPanes((all) => all.filter((x) => x.id !== p.id))}
            />
          ))}
        </div>
      </Section>
    </div>
  );
}

function Pane({ token, command, onClose }: { token: string; command: string; onClose: () => void }) {
  const t = useT();
  /**
   * The dictionary, reachable from the effect without being one of its dependencies.
   *
   * It was a dependency for one day, for the sake of one label, and the cost was absurd:
   * switching the language re-ran the effect, which closed the socket, which made the server
   * kill the process. `Shift+L` in a window with an open ssh session ended that session
   * without asking. A label is not worth a teardown.
   */
  const labels = useRef(t);
  labels.current = t;
  const host = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<"opening" | "open" | "closed">("opening");
  const [why, setWhy] = useState<string | null>(null);

  useEffect(() => {
    const el = host.current;
    if (!el) return;

    const term = new Terminal({
      // Without this xterm builds no accessibility manager at all, and a screen reader gets
      // a textarea with a fixed English label and nothing else: no output, no prompt, no
      // exit. The cost is a live region kept in step with the screen, which is the price of
      // the terminal being usable at all by somebody who cannot see it.
      screenReaderMode: true,
      convertEol: false,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      fontSize: 12,
      theme: theme(),
      // The program decides when to scroll; a terminal that also scrolls on its own fights
      // full-screen applications for the cursor.
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();

    // Same origin as the page, so `connect-src 'self'` covers it and no CSP exception is
    // needed for the socket.
    const url = new URL("/api/v1/terminal", location.href);
    url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";

    ws.onopen = () => {
      ws.send(JSON.stringify({ token, cols: term.cols, rows: term.rows, command }));
      setState("open");
      term.focus();
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        // The only text frames are ours, and they are refusals.
        try {
          const msg = JSON.parse(ev.data);
          if (msg.type === "error") setWhy(msg.message);
          // Not an error: a program that ran and finished. Shown so a pane that stops says
          // why rather than leaving the person to guess whether it crashed.
          if (msg.type === "exit") term.writeln(`\r\n[${labels.current.terminals.exited(msg.code)}]`);
        } catch {
          /* not ours; ignore rather than print protocol noise into the terminal */
        }
        return;
      }
      term.write(new Uint8Array(ev.data));
    };
    ws.onclose = () => setState("closed");
    ws.onerror = () => setState("closed");

    const send = term.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) ws.send(new TextEncoder().encode(data));
    });

    const resize = new ResizeObserver(() => {
      // A hidden element has no size, and fitting to no size tells the program inside that
      // its window is one column wide. The screen is kept mounted while the person is
      // looking at something else — see App.tsx — so this fires with nothing to fit, and a
      // shell that has been told the window is 1x1 draws nonsense when it comes back.
      if (el.clientWidth === 0 || el.clientHeight === 0) return;
      fit.fit();
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
      }
    });
    resize.observe(el);

    return () => {
      resize.disconnect();
      send.dispose();
      ws.close();
      term.dispose();
    };
  }, [token, command]);

  return (
    <div className="border rounded">
      <div className="flex items-center gap-2 px-2 py-1 border-b text-2xs">
        <span className="font-mono text-fg-muted flex-1 truncate">
          {command || t.terminals.shell}
        </span>
        <span className="text-fg-faint">{t.terminals.state[state]}</span>
        <button type="button" className="text-fg-faint hover:text-fg" onClick={onClose}>
          {t.terminals.close}
        </button>
      </div>
      {why ? (
        <p role="alert" className="px-2 py-1 text-2xs text-danger">
          {why}
        </p>
      ) : null}
      <div
        ref={host}
        className="h-72"
        role="group"
        aria-label={t.terminals.paneLabel(command || t.terminals.shell)}
      />
    </div>
  );
}

/**
 * A first guess at a name: the host for an `ssh`, otherwise the program.
 *
 * A guess and not a rule — the box it goes in is editable, and somebody naming a connection
 * "the old build server" knows better than any heuristic here.
 */
function defaultName(command: string): string {
  const parts = command.trim().split(/\s+/);
  const host = parts.find((p) => p.includes("@"));
  if (host) return host.split("@").pop() ?? host;
  return parts[0]?.split(/[\\/]/).pop()?.replace(/\.exe$/i, "") ?? "";
}
