import { describe, expect, it } from "vitest";
import { parseRoute } from "./router";

describe("parseRoute", () => {
  it("decodes a note name", () => {
    expect(parseRoute("#/note/caf%C3%A9%20noir").param).toBe("café noir");
  });

  it("does not throw on a broken escape", () => {
    // `#/note/%` reached the first render through a hand-made or truncated link and, with
    // no error boundary, left a blank page.
    expect(() => parseRoute("#/note/%")).not.toThrow();
    expect(parseRoute("#/note/%E0%A4%A").param).toBe("%E0%A4%A");
  });
});
