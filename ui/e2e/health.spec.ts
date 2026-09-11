import { expect, test } from "@playwright/test";
import { serve, type Serving } from "./serve";

/**
 * The health line under the question box, when the status call does not come back.
 *
 * It used to read "no answer" as "healthy": the check only ran when there was data to check,
 * so a failed call left a green dot and "Everything is in order." A line that exists to say
 * whether something is wrong must not say the opposite when it cannot tell.
 */

let running: Serving;

test.beforeAll(async () => {
  running = await serve();
});
test.afterAll(() => running?.stop());

test("a status call that fails is not reported as everything in order", async ({ page }) => {
  await page.route(/\/api\/v1\/status$/, (r) =>
    r.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ code: "internal", message: "status unavailable", exit_code: 2 }),
    }),
  );
  await page.goto(running.url);
  await expect(page.getByRole("textbox", { name: /your question/i })).toBeVisible();

  // Wait for the check to finish, so the assertion below is not satisfied by "checking…".
  await expect(page.getByText("checking…")).toHaveCount(0);
  await expect(page.locator("body")).not.toContainText("Everything is in order.");
  await expect(page.getByText("The state could not be checked.")).toBeVisible();
});

test("a status call that succeeds still says everything is in order", async ({ page }) => {
  await page.goto(running.url);
  await expect(page.getByText("Everything is in order.")).toBeVisible();
});
