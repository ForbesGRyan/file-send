import { test, expect } from "@playwright/test";
import { connectPair } from "../fixtures/peers";
import { bytes, sendFile } from "../fixtures/files";

test("receiver cancels an in-progress download", async ({ browser }) => {
  const { a, b, close } = await connectPair(browser);
  const content = bytes(48 * 1024 * 1024, 3); // big enough to still be active

  await sendFile(a, "cancel-me.bin", content);
  await b.locator("button.accept").click();
  // While bytes are flowing, B cancels.
  await b.locator("button.cancel").click();

  await expect(b.locator("li.row.declined .pct")).toHaveText("✗ CANCELLED");
  await close();
});

test("sender cancels an in-progress upload (no stuck Active row)", async ({ browser }) => {
  const { a, b, close } = await connectPair(browser);
  const content = bytes(48 * 1024 * 1024, 4);

  await sendFile(a, "cancel-send.bin", content);
  await b.locator("button.accept").click();
  // Sender A cancels its own upload while it streams.
  await a.locator("button.cancel").click();

  // The sender's row must settle on CANCELLED, never freeze mid-percent on Active.
  await expect(a.locator("li.row.declined .pct")).toHaveText("✗ CANCELLED");
  await close();
});
