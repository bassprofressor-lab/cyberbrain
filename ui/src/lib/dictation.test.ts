import { describe, expect, it } from "vitest";
import { appendDictation } from "@/lib/dictation";

describe("appendDictation", () => {
  it("takes the spoken text as it is into an empty field", () => {
    expect(appendDictation("  ", " Hallo Welt. ", true)).toBe("Hallo Welt.");
  });
  it("starts a new line in a text, and a space in a single line", () => {
    expect(appendDictation("Erste Zeile", "Zweite", true)).toBe("Erste Zeile\nZweite");
    expect(appendDictation("Erste Zeile\n", "Zweite", true)).toBe("Erste Zeile\nZweite");
    expect(appendDictation("redis", "upgrade", false)).toBe("redis upgrade");
  });
  it("leaves the field alone when nothing was heard", () => {
    expect(appendDictation("bleibt", "   ", true)).toBe("bleibt");
  });
});
