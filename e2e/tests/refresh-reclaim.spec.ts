import { test, expect } from "@playwright/test";
import { connectPair } from "../fixtures/peers";

const CONNECTED = "Connected — ready to transfer";

test("owner refresh while waiting alone reclaims the same room", async ({ browser }) => {
  const ctx = await browser.newContext();
  const a = await ctx.newPage();
  await a.goto("/");
  const code = await a.locator(".code").innerText();
  await expect(a.locator(".status")).toHaveText("Waiting for peer to join…");

  // Reload: the room was torn down when the socket dropped; the owner reclaims it.
  await a.reload();
  await expect(a.locator(".code")).toHaveText(code, { timeout: 20_000 });
  await expect(a.locator(".status")).toHaveText("Waiting for peer to join…");

  await ctx.close();
});

test("joiner mid-session refresh reconnects", async ({ browser }) => {
  const { a, b, code, close } = await connectPair(browser);

  await b.reload();
  // After B comes back, the room hash drives a re-Join and the pair reconnects.
  await expect(a.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });
  await expect(b.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });
  expect(b.url()).toContain(`/#/room/${code}`);

  await close();
});

test("owner mid-session refresh reconnects", async ({ browser }) => {
  const { a, b, close } = await connectPair(browser);

  await a.reload();
  await expect(a.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });
  await expect(b.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });

  await close();
});

test("a genuinely missing room shows recovery", async ({ page }) => {
  // Join a room code that was never created.
  await page.goto("/#/room/zzzzzz");
  await expect(page.locator(".status")).toHaveText("Room not found or expired");
  await expect(page.locator("button.newroom")).toBeVisible();
});
