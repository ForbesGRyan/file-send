import { test, expect } from "@playwright/test";
import { connectPair } from "../fixtures/peers";

test("a fresh visit creates a room and shows a code", async ({ page }) => {
  await page.goto("/");
  // The creating peer is assigned a room code and waits for a partner.
  await expect(page.locator(".code")).toHaveText(/^[a-z2-9]{6}$/);
  await expect(page.locator(".status")).toHaveText("Waiting for peer to join…");
});

test("two peers complete the WebRTC handshake", async ({ browser }) => {
  const pair = await connectPair(browser);
  // connectPair already asserts both sides reach Connected; assert the join UI
  // is gone on the connected side as a second signal the handshake finished.
  await expect(pair.a.locator(".joinbox")).toHaveCount(0);
  await pair.close();
});
