/**
 * Start a real `cyberbrain serve --terminal` over a throwaway store, and hand back the
 * address that carries the terminal token.
 *
 * The binary is the one this checkout builds. If it is missing the tests say so rather than
 * silently testing nothing — a suite that passes because it never ran is worse than none.
 */
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const BIN = resolve(import.meta.dirname, "../../target/debug/cyberbrain");

export interface Serving {
  url: string;
  store: string;
  stop: () => void;
}

/** A note to put in the store before the server starts. */
export interface Seed {
  ring: number;
  kind: string;
  name: string;
  body: string;
}

/**
 * `seed` is written with the CLI *before* `serve` starts, not after: the server indexes the
 * store it is given at startup, and a test that wrote afterwards would be asserting on
 * whatever the running process happened to notice.
 */
export async function serve(seed: Seed[] = []): Promise<Serving> {
  if (!existsSync(BIN)) {
    throw new Error(`${BIN} is not built; run \`cargo build -p cyberbrain\` first`);
  }
  const dir = mkdtempSync(join(tmpdir(), "cyberbrain-e2e-"));
  const store = join(dir, ".cyberbrain");
  await once(spawn(BIN, ["init", "--path", store]));
  for (const n of seed) {
    await once(spawn(BIN, ["--store", store, "write", "--ring", String(n.ring), "--kind", n.kind, "--name", n.name, "--body", n.body]));
  }

  const child = spawn(BIN, ["--store", store, "serve", "--port", "0", "--no-open", "--terminal"]);
  const url = await new Promise<string>((ok, fail) => {
    let seen = "";
    const timer = setTimeout(() => fail(new Error(`no address in:\n${seen}`)), 20_000);
    child.stdout.on("data", (b: Buffer) => {
      seen += b.toString();
      // The second address is the one with the token; the first is the plain one.
      const m = seen.match(/http:\/\/127\.0\.0\.1:\d+\/#\/\?t=[0-9a-f]+/);
      if (m) {
        clearTimeout(timer);
        ok(m[0]);
      }
    });
    child.on("exit", (code) => fail(new Error(`serve exited with ${code}:\n${seen}`)));
  });

  return { url, store, stop: () => void child.kill() };
}

function once(child: ChildProcess): Promise<void> {
  return new Promise((ok, fail) =>
    child.on("exit", (code) => (code === 0 ? ok() : fail(new Error(`exited ${code}`)))),
  );
}
