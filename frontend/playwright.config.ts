import { defineConfig, devices } from "@playwright/test";

/**
 * E2E tests run against the production backend serving the built frontend.
 * TASKER_E2E_URL overrides the target for local runs.
 */
export default defineConfig({
  testDir: "./e2e",
  timeout: 60_000,
  expect: {
    timeout: 10_000,
    // Phase 10 visual baselines: 1% pixel tolerance, animations disabled so
    // pulse skeletons and transitions cannot flake the comparison.
    toHaveScreenshot: { animations: "disabled", maxDiffPixelRatio: 0.01 },
  },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: process.env.TASKER_E2E_URL ?? "http://localhost:8091",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
