import { test, expect } from "@playwright/test";
import { bytes, sendFile, expectDownloadMatches } from "../fixtures/files";

const CONNECTED = "Connected — ready to transfer";
const RECONNECTING = "Reconnecting…";

test("peer reconnects after its signaling WebSocket drops", async ({ browser }) => {
  test.setTimeout(60_000);
  const ctxA = await browser.newContext({ acceptDownloads: true });
  const ctxB = await browser.newContext({ acceptDownloads: true });
  const a = await ctxA.newPage();
  const b = await ctxB.newPage();

  // Proxy B's signaling WS so we can drop it mid-session and count reconnects.
  // Each /ws connection B opens runs this handler, so bRoutes counts (re)opens.
  let bRoutes = 0;
  let lastWs: import("@playwright/test").WebSocketRoute | null = null;
  let lastServer: import("@playwright/test").WebSocketRoute | null = null;
  await b.routeWebSocket(/\/ws$/, (ws) => {
    bRoutes++;
    const server = ws.connectToServer();
    lastWs = ws;
    lastServer = server;
    ws.onMessage((m) => server.send(m));
    server.onMessage((m) => ws.send(m));
  });

  await a.goto("/");
  const code = await a.locator(".code").innerText();
  await b.goto(`/#/room/${code}`);
  await expect(a.locator(".status")).toHaveText(CONNECTED);
  await expect(b.locator(".status")).toHaveText(CONNECTED);
  expect(bRoutes).toBe(1);

  // Drop B's signaling WS: free the server slot AND close the client side so
  // B's page fires `onclose`.
  lastServer!.close();
  lastWs!.close();

  // B surfaces Reconnecting, re-opens the socket (bRoutes >= 2), then both
  // sides recover to Connected.
  await expect(b.locator(".status")).toHaveText(RECONNECTING, { timeout: 10_000 });
  await expect.poll(() => bRoutes, { timeout: 20_000 }).toBeGreaterThanOrEqual(2);
  await expect(b.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });
  await expect(a.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });

  // A transfer works after recovery.
  const content = bytes(64 * 1024, 9);
  await sendFile(a, "after-reconnect.bin", content);
  await expectDownloadMatches(b, content, () => b.locator("button.accept").click());

  await ctxA.close();
  await ctxB.close();
});
