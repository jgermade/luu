// @ts-check
import { defineConfig, devices } from "@playwright/test"

/**
 * The live server's own config, separate from `playwright.config.js` because
 * the two suites want different servers: that one serves the assembled static
 * twin out of `site/` and this one spawns `luu serve` from inside the spec,
 * with its own port and its own state directory. One config with both would
 * start the static server for a test that never looks at it — and would refuse
 * to run at all on a checkout where `site/` has not been assembled.
 *
 * See `RECORD/2026-09-08.a-test-that-clicks-approve.completed.md`.
 */
export default defineConfig({
  testDir: ".",
  testMatch: "gate.spec.js",
  // A whole job — plan, approval, tool call, fold — over a mock with no delay.
  timeout: 120_000,
  // A smoke test that passes on the second attempt is a smoke test that failed.
  retries: 0,
  reporter: process.env.CI ? "github" : "list",
  use: { trace: "retain-on-failure" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
})
