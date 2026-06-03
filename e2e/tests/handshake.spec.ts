import { test, expect } from "@playwright/test";

test("a fresh visit creates a room and shows a code", async ({ page }) => {
  await page.goto("/");
  // The creating peer is assigned a room code and waits for a partner.
  await expect(page.locator(".code")).toHaveText(/^[a-z2-9]{6}$/);
  await expect(page.locator(".status")).toHaveText("Waiting for peer to join…");
});
