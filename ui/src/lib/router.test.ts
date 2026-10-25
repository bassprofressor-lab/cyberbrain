import { describe, expect, it } from "vitest";
import { cameFromSearch, filterFromRoute, href, parseRoute } from "./router";

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

describe("filterFromRoute", () => {
  it("takes the tag from a tag link, also when the list is already showing", () => {
    // The click that did nothing: #/note/x -> #/notes?q=tag with the list left mounted.
    expect(filterFromRoute(parseRoute(href("notes", null, { q: "postgres" })), "")).toBe("postgres");
    expect(filterFromRoute(parseRoute(href("notes", null, { q: "postgres" })), "old")).toBe("postgres");
  });
  it("leaves what was typed alone on other addresses", () => {
    expect(filterFromRoute(parseRoute(href("note", "some-note")), "typed")).toBe("typed");
    expect(filterFromRoute(parseRoute(href("notes")), "typed")).toBe("typed");
  });
});

describe("cameFromSearch", () => {
  it("recognises the search address with or without a query", () => {
    expect(cameFromSearch("#/search?q=x")).toBe(true);
    expect(cameFromSearch("#/search")).toBe(true);
    expect(cameFromSearch("#/notes")).toBe(false);
    expect(cameFromSearch("")).toBe(false);
  });
});
