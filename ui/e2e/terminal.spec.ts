import { expect, test } from "@playwright/test";
import { serve, type Serving } from "./serve";

let running: Serving;

test.beforeAll(async () => {
  running = await serve();
});
test.afterAll(() => running?.stop());

test("the page takes the token out of the address and keeps it", async ({ page }) => {
  await page.goto(running.url);
  await expect(page).toHaveURL(/#\/status$/);
  // Away and back: the token has to survive navigation, or the terminal is gone after one
  // click and nobody can tell why.
  await page.getByRole("link", { name: /^Search/ }).click();
  await page.getByRole("link", { name: /^Terminal/ }).click();
  await expect(page.getByText(/carries no terminal token/i)).toHaveCount(0);
});

test("a shell opens and answers", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("link", { name: /^Terminal/ }).click();
  await page.getByRole("button", { name: "Shell", exact: true }).click();
  await page.keyboard.type("echo from-the-browser\n");
  await expect(page.locator(".xterm-rows")).toContainText("from-the-browser");
});

/**
 * Written against a defect that shipped: the effect that owns the socket had the i18n
 * dictionary in its dependency list, so switching the language tore the socket down and the
 * server killed the process behind it. One keystroke, and an ssh session was gone.
 */
test("switching the language does not kill a running terminal", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("link", { name: /^Terminal/ }).click();
  await page.getByRole("button", { name: "Shell", exact: true }).click();
  await page.keyboard.type("echo before-the-switch\n");
  await expect(page.locator(".xterm-rows")).toContainText("before-the-switch");

  // The button in the sidebar, which is the way a person does it. `Shift+L` is a global
  // shortcut and does not fire while the focus is inside the terminal — xterm takes the key,
  // and the shortcut is not registered for inputs. That is its own question; this test is
  // about what happens when the language does change.
  await page.getByRole("button", { name: /language/i }).click();
  await expect(page.getByRole("link", { name: /^Suche/ })).toBeVisible();

  // The same session, still there and still answering. Focus goes to xterm's own textarea:
  // `.xterm-rows` is `aria-hidden` and sits under `.xterm-screen`, so clicking it is a click
  // on something the browser will not give focus to.
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("echo after-the-switch\n");
  await expect(page.locator(".xterm-rows")).toContainText("after-the-switch");
  await expect(page.locator(".xterm-rows")).toContainText("before-the-switch");
});

test("the console runs a command against this store", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("link", { name: /^Command line/ }).click();
  await page.getByRole("textbox", { name: /Command/i }).fill("doctor");
  await page.keyboard.press("Enter");
  await expect(page.locator("pre")).toContainText("checks");
});
