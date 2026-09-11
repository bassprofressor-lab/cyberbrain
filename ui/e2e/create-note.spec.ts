import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { serve, type Serving } from "./serve";

/**
 * Writing a new note from the page.
 *
 * The server has taken POST /notes all along; the page never called it, so somebody who
 * wanted to write a thing down was sent to a command line. What is checked here is what a
 * person relies on without seeing it: the note lands on disk, an umlaut in the title does not
 * get the write refused, a second note with the same name does not overwrite the first, and
 * the page offers no way to put a note into rings 0 or 1, which bind every agent on the
 * machine.
 */

let running: Serving;

test.beforeAll(async () => {
  running = await serve();
});
test.afterAll(() => running?.stop());

const noteFile = (ring: number, name: string) => join(running.store, "notes", `r${ring}`, `${name}.md`);

test("the simple view writes a note into ring 2, under a name made from the title", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("link", { name: /^Notes/ }).click();
  await page.getByRole("button", { name: /new note/i }).first().click();

  await page.getByRole("textbox", { name: /^title/i }).fill("Auslieferung für Kunden");
  // Names are ASCII slugs; the title is not, and must not have to be.
  await expect(page.getByRole("textbox", { name: /short name/i })).toHaveValue("auslieferung-fuer-kunden");
  await page.getByRole("textbox", { name: /^text/i }).fill("Kunden bekommen eine Auslieferung erst nach der Freigabe.");
  await page.getByRole("button", { name: /^save/i }).click();

  await expect(page.getByRole("heading", { level: 1 })).toContainText(/auslieferung fuer kunden/i);
  expect(existsSync(noteFile(2, "auslieferung-fuer-kunden"))).toBe(true);
  expect(readFileSync(noteFile(2, "auslieferung-fuer-kunden"), "utf8")).toContain("erst nach der Freigabe");
});

test("a second note with the same name is refused and the first stays as it was", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("link", { name: /^Notes/ }).click();
  await page.getByRole("button", { name: /new note/i }).first().click();

  await page.getByRole("textbox", { name: /^title/i }).fill("Auslieferung für Kunden");
  await page.getByRole("textbox", { name: /^text/i }).fill("Das hier darf die erste Notiz nicht ersetzen.");
  await page.getByRole("button", { name: /^save/i }).click();

  await expect(page.getByRole("alert")).toContainText(/already/i);
  expect(readFileSync(noteFile(2, "auslieferung-fuer-kunden"), "utf8")).not.toContain("nicht ersetzen");
});

test("the full view creates in rings 2 to 4 and offers nothing above them", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("button", { name: /full view/i }).click();
  await page.getByRole("link", { name: /^Notes/ }).click();
  await page.getByRole("button", { name: /new note/i }).click();

  // Scoped to the form: the note list beside it has a kind filter of its own.
  const form = page.getByRole("form", { name: /new note/i });
  const ring = form.getByRole("combobox", { name: /^ring/i });
  const offered = await ring.locator("option").evaluateAll((os) => os.map((o) => (o as HTMLOptionElement).value));
  expect(offered).toEqual(["2", "3", "4"]);

  await form.getByRole("textbox", { name: /^title/i }).fill("Freitag ist Lesetag");
  await ring.selectOption("3");
  await form.getByRole("combobox", { name: /^kind/i }).selectOption("lesson");
  await form.getByRole("textbox", { name: /note body/i }).fill("Am Freitag wird nichts ausgeliefert, nur gelesen.");
  await form.getByRole("button", { name: /write to disk/i }).click();

  await expect(page.getByRole("heading", { level: 1 })).toHaveText("freitag-ist-lesetag");
  expect(existsSync(noteFile(3, "freitag-ist-lesetag"))).toBe(true);
});
