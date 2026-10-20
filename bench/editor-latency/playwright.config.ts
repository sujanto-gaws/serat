// NFR-PRF-02 budget harness config. Task #11 (P1/test).
//
// Chromium is not downloaded through Playwright's own installer here — this
// environment provisions it once at a fixed path, and `executablePath`
// points straight at it so the harness never triggers a network fetch (that
// would itself be an NFR-SEC-01/02 violation in spirit: a test harness has
// no business phoning out). Override with PW_CHROMIUM_PATH if this ever
// needs to run somewhere the browser lives elsewhere.
import { defineConfig, devices } from "@playwright/test";

const chromiumPath =
  process.env.PW_CHROMIUM_PATH ?? "/opt/pw-browsers/chromium";

export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  fullyParallel: false, // latency sampling shares one browser instance deliberately
  retries: 0, // a budget harness that retries away a slow run is not a budget harness
  reporter: [["list"], ["json", { outputFile: "out/results.json" }]],
  use: {
    trace: "off", // tracing screenshots load the page being timed
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        launchOptions: {
          executablePath: chromiumPath,
        },
      },
    },
  ],
});
