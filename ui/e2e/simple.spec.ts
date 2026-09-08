import { expect, test } from "@playwright/test";
import { serve, type Serving } from "./serve";

/**
 * The view a colleague gets, against a real server.
 *
 * What is checked here is not layout. It is the three claims the mode is built on: that it
 * is what an unconfigured machine opens with, that a question comes back as one answer
 * rather than a ranked list, and that none of the vocabulary the full view needs — ring
 * numbers, citations, scores — reaches this screen. Each of those has a `tsc`-invisible way
 * of quietly stopping being true.
 */

let running: Serving;

test.beforeAll(async () => {
  running = await serve([
    { ring: 0, kind: "decision", name: "no-release-on-a-friday", body: "Nothing is released on a Friday. An exception is agreed before the work starts, not after." },
    { ring: 2, kind: "knowledge", name: "who-signs-off-a-release", body: "A release is signed off by somebody who did not build it. Two pairs of eyes, and the second pair is not optional." },
  ]);
});
test.afterAll(() => running?.stop());

test("an unconfigured machine opens in the simple view, on the question box", async ({ page }) => {
  await page.goto(running.url);
  await expect(page).toHaveURL(/#\/search$/);
  await expect(page.getByRole("textbox", { name: /your question/i })).toBeVisible();

  // Three entries, and none of the six that only mean something to an operator.
  await expect(page.locator("nav a")).toHaveCount(3);
  await expect(page.getByRole("link", { name: /^Compliance/ })).toHaveCount(0);
  await expect(page.getByRole("link", { name: /^Command line/ })).toHaveCount(0);
  // The `g s` hints belong to a keyboard the reader does not have yet.
  await expect(page.locator("nav")).not.toContainText("g s");
});

test("a question comes back as one answer, with the sources under it", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("textbox", { name: /your question/i }).fill("can we release on a friday");

  // The ring-0 note is the answer, not the first row of a list: it is the one thing in an
  // <article>, and the tier is named in words rather than as r0.
  const answer = page.locator("article");
  await expect(answer).toContainText("Nothing is released on a Friday");
  await expect(answer).toContainText("Always applies");
  await expect(answer).toHaveCount(1);

  // The second note is a source under it, not a competing answer.
  await expect(page.getByText(/Where this comes from/i)).toBeVisible();
  await expect(page.getByText(/did not build it/)).toBeVisible();
});

test("no citation, ring number or score reaches the simple view", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("textbox", { name: /your question/i }).fill("can we release on a friday");
  await expect(page.locator("article")).toContainText("Nothing is released on a Friday");

  // A citation is the one string that is unmistakably from the other view.
  await expect(page.locator("main")).not.toContainText(/r[0-4]-[0-9a-f]{12}/);
  await expect(page.locator("main")).not.toContainText(/RRF|k_lex|hybrid/);
});

test("the full view is one switch away, and is remembered", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("button", { name: /full view/i }).click();
  await expect(page.getByRole("link", { name: /^Compliance/ })).toBeVisible();

  // Remembered, or an operator flips it again every morning.
  await page.reload();
  await expect(page.getByRole("link", { name: /^Compliance/ })).toBeVisible();

  await page.getByRole("button", { name: /simple view/i }).click();
  await expect(page.locator("nav a")).toHaveCount(3);
});

test("opening a source lands on the note, not in the editor", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("textbox", { name: /your question/i }).fill("can we release on a friday");
  await page.getByRole("button", { name: /open the note/i }).click();

  await expect(page).toHaveURL(/#\/note\/no-release-on-a-friday/);
  // The name is a file name; the heading is not.
  await expect(page.getByRole("heading", { name: "No release on a friday" })).toBeVisible();
  await expect(page.getByRole("button", { name: /^edit/i })).toHaveCount(0);
});

/**
 * The first day on a machine, which is the state every new install is in. The full view says
 * "no note matches", which is what a filter says and reads as one.
 */
test("an empty store says it is empty, and says how that changes", async ({ page }) => {
  const fresh = await serve();
  try {
    await page.goto(fresh.url);
    await page.getByRole("link", { name: /^Notes/ }).click();
    await expect(page.getByText(/Nothing written down yet/i)).toBeVisible();
    await expect(page.getByText(/no note matches/i)).toHaveCount(0);

    await page.getByRole("button", { name: /How does something get here/i }).click();
    await expect(page.getByText("cyberbrain write --ring 2")).toBeVisible();
  } finally {
    fresh.stop();
  }
});
