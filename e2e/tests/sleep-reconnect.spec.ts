import { test, expect } from "@playwright/test";
import { connectPair } from "../fixtures/peers";
import { bytes, sendFile, expectDownloadMatches } from "../fixtures/files";

const CONNECTED = "Connected — ready to transfer";

// KNOWN GAP: a peer going offline (laptop sleep / NIC drop) and coming back does
// not reliably re-establish the connection. `setOffline` simulates the drop.
// Remove `.fixme` once sleep/reconnect recovery is fixed.
test.fixme("peer recovers after going offline and back", async ({ browser }) => {
  const { a, b, close } = await connectPair(browser);

  // B's context goes offline (simulated sleep), then back online.
  const ctxB = b.context();
  await ctxB.setOffline(true);
  // Give the server's socket-close + PeerLeft path time to fire.
  await a.waitForTimeout(5_000);
  await ctxB.setOffline(false);

  // "Recovered" means: both sides return to Connected AND a fresh transfer works.
  await expect(a.locator(".status")).toHaveText(CONNECTED, { timeout: 30_000 });
  await expect(b.locator(".status")).toHaveText(CONNECTED, { timeout: 30_000 });

  const content = bytes(256 * 1024, 9);
  await sendFile(a, "after-wake.bin", content);
  await expectDownloadMatches(b, content, () => b.locator("button.accept").click());

  await close();
});
