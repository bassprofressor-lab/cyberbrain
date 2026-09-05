/**
 * The one seam. Everything in the UI imports `api` from here and nothing else knows
 * whether it is talking to the binary or to the mock.
 *
 * Switching to the real backend is either
 *   • `VITE_API=http npm run build`     (build-time env), or
 *   • flip DEFAULT_TRANSPORT below.
 *
 * Until `cyberbrain serve` exists the default is the mock, and the UI shows a badge saying
 * so — a compliance screen full of fabricated zeros must never look real.
 */
import type { CyberbrainApi } from "./types";
import { httpClient } from "./http";
import { mockClient } from "./mock";

// Real data by default. Mock has to be asked for.
//
// This was the other way round, and it is the one default in this project that could ship
// a lie: a release binary embeds `dist/`, and a mock bundle renders fabricated compliance
// numbers — "nothing has left this machine", audit rows, egress counts — in a screen whose
// entire purpose is to be believed. The MOCK DATA badge is a good guard for a developer
// looking at the page and no guard at all for a binary handed to somebody else.
//
// `npm run build:mock` still produces the mock bundle for UI work.
const DEFAULT_TRANSPORT: "mock" | "http" = "http";

const transport = (import.meta.env.VITE_API as string | undefined) === "http" ? "http" : (import.meta.env.VITE_API as string | undefined) === "mock" ? "mock" : DEFAULT_TRANSPORT;

export const api: CyberbrainApi = transport === "http" ? httpClient : mockClient;

export * from "./types";
