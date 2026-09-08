import { expect, test } from "@playwright/test";
import { serve, type Serving } from "./serve";

let running: Serving;

test.beforeAll(async () => {
  running = await serve();
});
test.afterAll(() => running?.stop());

/**
 * These are tests of the full view: its sidebar, its landing screen, its terminal. The page
 * now opens in simple mode unless told otherwise, so each of them says so before the first
 * paint rather than clicking the switch and racing the render.
 */
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("cyberbrain.mode", "expert"));
});

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


/**
 * Written against a defect that shipped: the panes lived in the screen's own state, so
 * clicking any other entry in the sidebar unmounted it, closed the sockets, and the server
 * killed the processes behind them. An ssh session ended because somebody looked at Status.
 */
test("going to another screen and back leaves a terminal running", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("link", { name: /^Terminal/ }).click();
  await page.getByRole("button", { name: "Shell", exact: true }).click();
  await page.keyboard.type("echo still-here\n");
  await expect(page.locator(".xterm-rows")).toContainText("still-here");

  await page.getByRole("link", { name: /^Status/ }).click();
  await expect(page.getByRole("link", { name: /^Terminal/ })).toBeVisible();
  await page.getByRole("link", { name: /^Terminal/ }).click();

  // The same session: what it printed before is still on its screen, and it still answers.
  await expect(page.locator(".xterm-rows")).toContainText("still-here");
  // Click the screen, the way a person does. Focusing the helper textarea directly races the
  // redraw that follows unhiding, and the first keystrokes land nowhere.
  await page.locator(".xterm-screen").click();
  await expect(page.locator(".xterm-helper-textarea")).toBeFocused();
  await page.keyboard.type("echo and-answering\n");
  await expect(page.locator(".xterm-rows")).toContainText("and-answering");
});

/**
 * The output is the whole of a terminal's answer, and xterm builds no accessibility manager
 * unless it is asked to — without it a screen reader gets a textarea and silence.
 */
test("the terminal is announced to a screen reader", async ({ page }) => {
  await page.goto(running.url);
  await page.getByRole("link", { name: /^Terminal/ }).click();
  await page.getByRole("button", { name: "Shell", exact: true }).click();
  await expect(page.getByRole("group", { name: /terminal running/i })).toBeVisible();
  // xterm builds its accessibility manager only in screen-reader mode, and the manager is
  // what creates this: the live region that a screen reader actually reads the output from.
  await expect(page.locator(".xterm-accessibility-tree")).toHaveCount(1);
});
