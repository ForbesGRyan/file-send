import { Browser, Page, expect } from "@playwright/test";

const CONNECTED = "Connected — ready to transfer";

/** A connected pair of peers, each on its own browser context. */
export interface Pair {
  a: Page;
  b: Page;
  code: string;
  close: () => Promise<void>;
}

/**
 * Open two independent browser contexts, have peer A create a room and peer B
 * join it by URL, and wait until both report Connected. Returns both pages plus
 * the room code and a teardown that closes both contexts.
 */
export async function connectPair(browser: Browser): Promise<Pair> {
  const ctxA = await browser.newContext({ acceptDownloads: true });
  const ctxB = await browser.newContext({ acceptDownloads: true });
  const a = await ctxA.newPage();
  const b = await ctxB.newPage();

  await a.goto("/");
  const code = await a.locator(".code").innerText();
  expect(code).toMatch(/^[a-z2-9]{6}$/);

  // B joins by navigating to the room hash (the same URL the share link uses).
  await b.goto(`/#/room/${code}`);

  await expect(a.locator(".status")).toHaveText(CONNECTED);
  await expect(b.locator(".status")).toHaveText(CONNECTED);

  return {
    a,
    b,
    code,
    close: async () => {
      await ctxA.close();
      await ctxB.close();
    },
  };
}
