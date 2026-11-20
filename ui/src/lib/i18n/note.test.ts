import { describe, expect, it } from "vitest";
import { de } from "./de";
import { en } from "./en";

// O5: the German note view must not show the English command word or the bare technical labels.
describe("note view wording (O5)", () => {
  it("has a German forget button", () => {
    expect(de.note.forget).not.toMatch(/^forget/i);
    expect(en.note.forget).toMatch(/^forget/i);
  });
  it("labels id and path in words", () => {
    expect(de.note.field.id).not.toBe("ID");
    expect(de.note.field.path).not.toBe("Pfad");
  });
  it("has a way back to the list on narrow screens in both languages", () => {
    expect(de.notes.toList).toBeTruthy();
    expect(en.notes.toList).toBeTruthy();
  });
});
