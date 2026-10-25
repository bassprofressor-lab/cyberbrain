import { describe, expect, it } from "vitest";
import { bodyWithTitle, slugFromTitle, splitTitle, titleOf } from "./slug";

describe("bodyWithTitle", () => {
  it("keeps a title the file name would lose", () => {
    const title = "Auslieferung am Freitag: ja oder nein?";
    const name = slugFromTitle(title);
    expect(bodyWithTitle(title, name, "Nein.")).toBe("# Auslieferung am Freitag: ja oder nein?\n\nNein.");
  });
  it("leaves the text alone when the name reads back as the title", () => {
    expect(bodyWithTitle("Keine auslieferung", "keine-auslieferung", "text")).toBe("text");
    expect(titleOf("keine-auslieferung")).toBe("Keine auslieferung");
  });
  it("does not add a second heading", () => {
    expect(bodyWithTitle("Ein Titel!", "ein-titel", "# Mein Titel\n\ntext")).toBe("# Mein Titel\n\ntext");
  });
  it("does nothing without a title", () => {
    expect(bodyWithTitle("  ", "x", "text")).toBe("text");
  });
});

describe("splitTitle", () => {
  it("shows a stored title once", () => {
    expect(splitTitle("x", "# Auslieferung: ja?\n\nNein.")).toEqual({ title: "Auslieferung: ja?", text: "Nein.", skipped: 2 });
  });
  it("falls back to the name when the body has no heading", () => {
    expect(splitTitle("keine-auslieferung", "text")).toEqual({ title: "Keine auslieferung", text: "text", skipped: 0 });
  });
  it("round-trips with bodyWithTitle", () => {
    const title = "Ist das ok, oder nicht?";
    const name = slugFromTitle(title);
    expect(splitTitle(name, bodyWithTitle(title, name, "Doch."))).toEqual({ title, text: "Doch.", skipped: 2 });
  });
});
