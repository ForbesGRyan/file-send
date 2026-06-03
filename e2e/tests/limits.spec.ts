import { test, expect } from "@playwright/test";
import { connectPair } from "../fixtures/peers";

test("a third peer is told the room is full", async ({ browser }) => {
  const { code, close } = await connectPair(browser);

  const ctxC = await browser.newContext();
  const c = await ctxC.newPage();
  await c.goto(`/#/room/${code}`);
  await expect(c.locator(".status")).toHaveText("Room is full");

  await ctxC.close();
  await close();
});

test("a bad code surfaces room-not-found", async ({ page }) => {
  await page.goto("/#/room/aaaaaa");
  await expect(page.locator(".status")).toHaveText("Room not found or expired");
});

// KNOWN GAP: symmetric-NAT direct-connection failure cannot be faithfully
// simulated on localhost loopback (no NAT, no TURN). Verifying that the UI
// surfaces a clear "couldn't establish direct connection" error needs real
// network infra or forced ICE-failure injection. Remove `.fixme` when such a
// harness exists (e.g. a TURN-less relay-only ICE config or netem-based setup).
test.fixme("symmetric-NAT failure surfaces a clear error", async () => {
  // Intentionally empty: documents an uncovered scenario so it is never mistaken
  // for covered. See docs/testing.md.
});
