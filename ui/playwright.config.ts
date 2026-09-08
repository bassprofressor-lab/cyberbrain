import { defineConfig } from "@playwright/test";

/**
 * Browser tests against a real `cyberbrain serve`.
 *
 * The page has parts that only a browser can be wrong about: a WebSocket that the content
 * security policy has to allow, a token that arrives in the URL fragment, and an effect
 * whose dependency list decides whether switching the language destroys somebody's session.
 * None of that is visible to `tsc`, and all of it has been wrong at least once.
 *
 * The server is started by the test itself (`e2e/serve.ts`) rather than by a `webServer`
 * entry, because each test needs the token that server prints on startup.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI ? "github" : "list",
  use: { headless: true },
});
