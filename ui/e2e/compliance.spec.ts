import { expect, test } from "@playwright/test";
import { serve, type Serving } from "./serve";

/**
 * The compliance overview, on a store as it is the day it was created.
 *
 * The page was written for two egress purposes while the server sent seven. Four of those
 * name their destination in words ("not enrolled", "wherever you point it"), the server
 * classified the words as an unresolved hostname, and the overview warns about those — so a
 * fresh store said "Nothing has left this machine." under a warning colour, with the
 * sentences listed as its own network.
 */

let running: Serving;

test.beforeAll(async () => {
  running = await serve();
});
test.afterAll(() => running?.stop());

test("a fresh store's overview says nothing left, without a warning", async ({ page }) => {
  await page.goto(running.url.replace("#/?", "#/compliance?"));
  const overview = page.locator(".panel").filter({ hasText: "Nothing has left this machine." });
  await expect(overview).toBeVisible();
  await expect(overview).not.toHaveAttribute("style", /--warn/);

  const ownNetwork = overview.getByText("to your own network").locator("..");
  await expect(ownNetwork).toContainText("127.0.0.1:11434");
  await expect(ownNetwork).not.toContainText("not enrolled");
  await expect(ownNetwork).not.toContainText("wherever you point it");
  await expect(ownNetwork).not.toContainText("unresolved");
});

