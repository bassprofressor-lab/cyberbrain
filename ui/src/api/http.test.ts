import { afterEach, describe, expect, it, vi } from "vitest";
import { httpClient } from "./http";
import { captureToken } from "@/lib/terminalToken";

function stubFetch() {
  const fetchMock = vi.fn(async () => new Response("{}", { status: 200 }));
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}
const headersOf = (m: ReturnType<typeof stubFetch>, call = 0) => ((m.mock.calls[call] as unknown as [string, RequestInit])[1].headers ?? {}) as Record<string, string>;

describe("terminal token header", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("sends no token header while there is no token", async () => {
    const m = stubFetch();
    await httpClient.status();
    expect(headersOf(m)["x-cyberbrain-terminal-token"]).toBeUndefined();
  });

  it("sends it on reads, decisions, holds and commands once there is one", async () => {
    captureToken(new URLSearchParams("t=geheim-123"));
    const m = stubFetch();
    await httpClient.status();
    await httpClient.decideProposal("p", true, "ok");
    await httpClient.resolveHold("h1", { action: "release" } as never);
    await httpClient.command("status");
    for (let i = 0; i < 4; i++) expect(headersOf(m, i)["x-cyberbrain-terminal-token"]).toBe("geheim-123");
    // the JSON content type must survive next to it
    expect(headersOf(m, 1)["Content-Type"]).toBe("application/json");
  });
});
