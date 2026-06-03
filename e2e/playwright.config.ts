import { defineConfig } from "@playwright/test";

const PORT = 3100;
const BASE_URL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  // WebRTC handshakes have real network timing; give assertions room.
  timeout: 60_000,
  expect: { timeout: 15_000 },
  // Retries only here (E2E), never in the Rust suite. Flake guard for timing.
  retries: process.env.CI ? 2 : 0,
  fullyParallel: false, // two-peer tests are heavier; keep them serial for stability
  workers: 1,
  reporter: process.env.CI ? "list" : "html",
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: {
    // Build the WASM client, then run the real server binary serving dist.
    command:
      "cd ../crates/client && trunk build && cd ../.. && cargo run --release -p server",
    url: BASE_URL,
    timeout: 180_000,
    reuseExistingServer: !process.env.CI,
    env: {
      BIND_ADDR: `127.0.0.1:${PORT}`,
      CLIENT_DIST: "crates/client/dist",
    },
  },
});
