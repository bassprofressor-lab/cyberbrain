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
            className="flex-1 bg-transparent font-mono text-xs outline-none border-b py-1 placeholder:text-fg-faint"
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
        </form>

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
  const host = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<"opening" | "open" | "closed">("opening");
  const [why, setWhy] = useState<string | null>(null);

  useEffect(() => {
    const el = host.current;
    if (!el) return;

    const term = new Terminal({
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

    const argv = splitCommand(command);
    ws.onopen = () => {
      ws.send(JSON.stringify({ token, cols: term.cols, rows: term.rows, command: argv }));
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
          if (msg.type === "exit") term.writeln(`\r\n[${t.terminals.exited(msg.code)}]`);
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
  }, [token, command, t]);

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
      {why ? <p className="px-2 py-1 text-2xs text-danger">{why}</p> : null}
      <div ref={host} className="h-72" />
    </div>
  );
}

/**
 * Split a typed command the way the person means it, and no further.
 *
 * Quotes group and a backslash escapes; there is no shell here, so a pipe or a semicolon is
 * an argument. Somebody who wants a pipe asks for a shell and types it there — which is
 * exactly what the "shell" button is for.
 */
export function splitCommand(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let has = false;
  let quote: string | null = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i] as string;
    if (c === "\\" && i + 1 < line.length) {
      cur += line[++i];
      has = true;
    } else if (quote && c === quote) {
      quote = null;
    } else if (quote) {
      cur += c;
      has = true;
    } else if (c === '"' || c === "'") {
      quote = c;
      has = true;
    } else if (/\s/.test(c)) {
      if (has) {
        out.push(cur);
        cur = "";
        has = false;
      }
    } else {
      cur += c;
      has = true;
    }
  }
  if (has) out.push(cur);
  return out;
}
