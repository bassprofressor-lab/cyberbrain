import { describe, expect, it } from "vitest";
import { errorKind } from "./errors";

const err = (status: number, code: string, message = "m") => ({ status, body: { code, message, exit_code: 1 } }) as Parameters<typeof errorKind>[0];

describe("errorKind", () => {
  it("recognises a server that did not answer", () => {
    expect(errorKind(err(0, "internal", "cyberbrain serve is not reachable (Failed to fetch)"))).toBe("unreachable");
  });
  it("keeps the server's own codes", () => {
    expect(errorKind(err(404, "not-found"))).toBe("not-found");
    expect(errorKind(err(409, "write-conflict"))).toBe("write-conflict");
    expect(errorKind(err(403, "policy-refusal"))).toBe("policy-refusal");
  });
  it("calls anything it does not know internal", () => {
    expect(errorKind(err(500, "something-new"))).toBe("internal");
  });
});
