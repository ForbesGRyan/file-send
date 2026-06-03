import { test, expect } from "@playwright/test";
import { connectPair } from "../fixtures/peers";
import { bytes, sendFile, expectDownloadMatches } from "../fixtures/files";

test("A->B small file transfers byte-exact", async ({ browser }) => {
  const { a, b, close } = await connectPair(browser);
  const content = bytes(64 * 1024, 11);

  await sendFile(a, "small.bin", content);
  // B sees the offer; accepting triggers the transfer + auto-download.
  await expectDownloadMatches(b, content, () => b.locator("button.accept").click());
  await expect(b.locator("li.row.done .pct")).toHaveText("✓ DONE");

  await close();
});

test("large file transfers byte-exact (exercises backpressure)", async ({ browser }) => {
  test.setTimeout(120_000);
  const { a, b, close } = await connectPair(browser);
  // > BUFFER_HIGH (8 MB) so the bufferedamountlow pacing path runs.
  const content = bytes(24 * 1024 * 1024, 7);

  await sendFile(a, "big.bin", content);
  await expectDownloadMatches(b, content, () => b.locator("button.accept").click());

  await close();
});

test("bidirectional transfer works", async ({ browser }) => {
  const { a, b, close } = await connectPair(browser);
  const aToB = bytes(128 * 1024, 1);
  const bToA = bytes(128 * 1024, 2);

  await sendFile(a, "a-to-b.bin", aToB);
  await expectDownloadMatches(b, aToB, () => b.locator("button.accept").click());

  await sendFile(b, "b-to-a.bin", bToA);
  await expectDownloadMatches(a, bToA, () => a.locator("button.accept").click());

  await close();
});
