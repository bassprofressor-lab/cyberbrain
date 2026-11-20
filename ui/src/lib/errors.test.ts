import { describe, expect, it } from "vitest";
import { errorKind, hasSkippedCheck, isNotFound } from "./errors";

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

describe("isNotFound (O4)", () => {
  it("is true for a 404 and for the not-found code, false for others", () => {
    expect(isNotFound(err(404, "not-found"))).toBe(true);
    expect(isNotFound(err(409, "write-conflict"))).toBe(false);
    expect(isNotFound(err(500, "internal"))).toBe(false);
  });
});

describe("hasSkippedCheck (O3)", () => {
  it("is true when a check did not run, false for plain operator remarks", () => {
    expect(hasSkippedCheck(["semantic search skipped: no embedder configured"])).toBe(true);
    expect(hasSkippedCheck(["conflict check disabled"])).toBe(true);
    expect(hasSkippedCheck(["embedder not loaded: no model artefact at /x"])).toBe(false);
    expect(hasSkippedCheck([])).toBe(false);
  });
});
