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

const DEFAULT_TRANSPORT: "mock" | "http" = "mock";

const transport = (import.meta.env.VITE_API as string | undefined) === "http" ? "http" : (import.meta.env.VITE_API as string | undefined) === "mock" ? "mock" : DEFAULT_TRANSPORT;

export const api: CyberbrainApi = transport === "http" ? httpClient : mockClient;

export * from "./types";
