import { Page, expect } from "@playwright/test";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

/** Deterministic pseudo-random bytes of length `n` (seeded, repeatable). */
export function bytes(n: number, seed = 1): Buffer {
  const out = Buffer.alloc(n);
  let x = seed >>> 0;
  for (let i = 0; i < n; i++) {
    // xorshift32 — cheap, deterministic, good enough to defeat trivial dedupe.
    x ^= x << 13; x >>>= 0;
    x ^= x >> 17;
    x ^= x << 5; x >>>= 0;
    out[i] = x & 0xff;
  }
  return out;
}

export function sha256(buf: Buffer): string {
  return createHash("sha256").update(buf).digest("hex");
}

/** Put a file with the given name/content into a peer's file input (offers it). */
export async function sendFile(page: Page, name: string, content: Buffer, mimeType = "application/octet-stream") {
  await page.locator('input[type=file]').setInputFiles({ name, mimeType, buffer: content });
}

/**
 * Run `trigger` (the action that causes the receiver to download — usually the
 * Accept click) and assert the resulting download's bytes hash-match `expected`.
 * The download listener is registered BEFORE `trigger` runs (via Promise.all)
 * so a fast small-file download can't fire before we're listening.
 */
export async function expectDownloadMatches(
  page: Page,
  expected: Buffer,
  trigger: () => Promise<void>,
) {
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    trigger(),
  ]);
  const path = await download.path();
  if (path === null) {
    throw new Error(
      "download.path() returned null — was acceptDownloads: true set on this context?",
    );
  }
  const got = await readFile(path);
  expect(sha256(got)).toBe(sha256(expected));
}
