/**
 * The command line, in the window.
 *
 * A transcript and an input, and deliberately nothing more. It is not a terminal emulator:
 * there is no cursor addressing, no colour, no job control and no history file, because the
 * thing being run is not interactive — every command answers once and exits. What it does
 * carry is the two things a terminal gives that a form does not: the exact line you typed,
 * and the exit code you got.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError } from "@/api/client";
import type { CommandResult } from "@/api/types";
import { Empty, Pill, Section } from "@/components/ui";
import { useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { toApiError, useAsync } from "@/lib/useAsync";

/** One line that was run, and what came back. Refusals are entries too: a refused command
 * is an answer, and hiding it would leave the transcript disagreeing with what happened. */
interface Entry {
  id: number;
  line: string;
  result: CommandResult | null;
  error: ApiError | null;
}

const EXAMPLES = ["status", "doctor", "find App", "scan", "policy egress"];

/**
 * The project this page belongs to, from the store path.
 *
 * Shown because of the side-by-side window: four command lines next to each other are four
 * identical boxes unless each says whose it is, and a command typed into the wrong one runs
 * against the wrong store. `store.path` ends in the store directory, and the folder holding
 * it is what people call the project.
 */
function projectOf(storePath: string | undefined): string | null {
  if (!storePath) return null;
  const parts = storePath.split("/").filter(Boolean);
  if (parts.length === 0) return null;
  const last = parts[parts.length - 1];
  if (last === ".cyberbrain" && parts.length >= 2) return parts[parts.length - 2] ?? null;
  return last ?? null;
}

export function ConsoleScreen() {
  const t = useT();
  const [line, setLine] = useState("");
  const [entries, setEntries] = useState<Entry[]>([]);
  const [busy, setBusy] = useState(false);
  /** What was typed, newest last. Separate from `entries` because a refused line is worth
   * recalling with the up arrow — that is usually the line you meant to fix. */
  const [history, setHistory] = useState<string[]>([]);
  const [at, setAt] = useState<number | null>(null);
  const status = useAsync(() => api.status(), []);
  const project = projectOf(status.data?.store.path);
  const input = useRef<HTMLInputElement>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const next = useRef(0);

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [entries, busy]);

  const clear = useCallback(() => setEntries([]), []);

  useShortcuts(
    "console",
    [
      { keys: "/", label: t.console.keys.focus, run: () => input.current?.focus() },
      { keys: "Mod+l", label: t.console.keys.clear, run: clear, inInputs: true },
    ],
    [t, clear],
  );

  async function run(raw: string) {
    const typed = raw.trim();
    if (!typed || busy) return;
    setBusy(true);
    setHistory((h) => (h[h.length - 1] === typed ? h : [...h, typed]));
    setAt(null);
    setLine("");
    const id = next.current++;
    try {
      const result = await api.command(typed);
      setEntries((e) => [...e, { id, line: typed, result, error: null }]);
    } catch (e) {
      setEntries((e) => [...e, { id, line: typed, result: null, error: toApiError(e) }]);
    } finally {
      setBusy(false);
      input.current?.focus();
    }
  }

  /** Up and down walk the history, the way they do at a prompt: leaving the top puts back
   * whatever was half-typed rather than an empty line. */
  const [draft, setDraft] = useState("");
  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") {
      e.preventDefault();
      void run(line);
      return;
    }
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    if (history.length === 0) return;
    e.preventDefault();
    if (e.key === "ArrowUp") {
      const i = at === null ? history.length - 1 : Math.max(0, at - 1);
      if (at === null) setDraft(line);
      setAt(i);
      setLine(history[i] ?? "");
    } else {
      if (at === null) return;
      const i = at + 1;
      if (i >= history.length) {
        setAt(null);
        setLine(draft);
      } else {
        setAt(i);
        setLine(history[i] ?? "");
      }
    }
  }

  return (
    <div className="h-full flex flex-col min-h-0">
      <Section
        title={
          <span className="flex items-baseline gap-2">
            {t.console.title}
            {project ? <span className="font-mono text-xs text-fg-faint">{project}</span> : null}
          </span>
        }
        aside={
          entries.length > 0 ? (
            <button type="button" className="text-2xs text-fg-faint hover:text-fg" onClick={clear}>
              {t.console.clear}
            </button>
          ) : undefined
        }
        className="flex-1 min-h-0 flex flex-col"
      >
        <p className="text-2xs text-fg-faint mb-3 max-w-prose">{t.console.lead}</p>

        <div
          className="flex-1 min-h-0 overflow-y-auto font-mono text-xs space-y-3 pr-1"
          // The output is the whole of the answer here. Without this a screen reader user
          // presses Enter and is told nothing at all.
          aria-live="polite"
          aria-atomic="false"
        >
          {entries.length === 0 && !busy ? (
            <Empty title={t.console.empty}>
              <div className="flex flex-wrap gap-1.5 mt-2">
                <span className="text-fg-faint">{t.console.tryThese}</span>
                {EXAMPLES.map((x) => (
                  <button key={x} type="button" className="underline decoration-dotted hover:text-fg" onClick={() => void run(x)}>
                    {x}
                  </button>
                ))}
              </div>
            </Empty>
          ) : null}

          {entries.map((e) => (
            <Transcript key={e.id} entry={e} />
          ))}
          {busy ? (
            <div role="status" className="text-fg-faint">
              {t.console.running}…
            </div>
          ) : null}
          <div ref={bottom} />
        </div>

        <div className="pt-3 mt-2 border-t flex items-center gap-2">
          <span aria-hidden className="font-mono text-fg-faint select-none">
            ›
          </span>
          <input
            ref={input}
            value={line}
            onChange={(ev) => setLine(ev.target.value)}
            onKeyDown={onKeyDown}
            aria-label={t.console.inputAria}
            placeholder={t.console.placeholder}
            spellCheck={false}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            // `readOnly`, not `disabled`. Disabling the focused element makes the browser
            // take the focus away, it falls to the body, and keystrokes during a slow
            // command then reach the global shortcuts — a typed `g t` navigates away
            // mid-command.
            readOnly={busy}
            className="flex-1 bg-transparent font-mono text-xs rounded-sm placeholder:text-fg-faint"
          />
          <button
            type="button"
            className="text-2xs text-fg-muted hover:text-fg disabled:opacity-50"
            onClick={() => void run(line)}
            disabled={busy || line.trim() === ""}
          >
            {t.console.run}
          </button>
        </div>
        <p className="text-2xs text-fg-faint mt-1">{t.console.history}</p>
      </Section>
    </div>
  );
}

function Transcript({ entry }: { entry: Entry }) {
  const t = useT();
  const { result, error } = entry;
  return (
    <div>
      <div className="flex items-baseline gap-2">
        <span aria-hidden className="text-fg-faint select-none">
          ›
        </span>
        <span className="flex-1 break-all">{entry.line}</span>
        {error ? (
          <Pill tone="danger">{t.console.refused}</Pill>
        ) : result && result.exit_code !== 0 ? (
          <Pill tone="warn">{t.console.exit(result.exit_code)}</Pill>
        ) : null}
      </div>
      {/* A refusal is the server's own sentence. It says which command and why, and
          paraphrasing it here would be a second place to keep that reason true. */}
      {error ? <pre className="mt-1 ml-4 whitespace-pre-wrap text-danger">{error.body.message}</pre> : null}
      {result?.stdout ? <pre className="mt-1 ml-4 whitespace-pre-wrap">{result.stdout}</pre> : null}
      {result?.stderr ? <pre className="mt-1 ml-4 whitespace-pre-wrap text-warn">{result.stderr}</pre> : null}
      {result?.truncated ? <div className="ml-4 text-2xs text-fg-faint">{t.console.truncated}</div> : null}
    </div>
  );
}
