// NFR-PRF-02 budget harness config. Task #11 (P1/test).
//
// The harness itself fetches nothing (a test harness has no business phoning out). In the development
// container the browser is provisioned once at a fixed path and `executablePath` points at it. In CI the
// `editor-latency` job runs `npx playwright install chromium` as a step OUTSIDE the harness, and the
// fallback below selects that install's FULL Chromium (channel "chromium", new headless), never the old
// `chromium-headless-shell` Playwright would otherwise pick: both report the same version string, so the
// browser mode is put on every result line (SERAT_BROWSER, read by tests/report.ts). Override with
// PW_CHROMIUM_PATH to run anywhere else.
import { existsSync } from "node:fs";
import { defineConfig, devices } from "@playwright/test";

const chromiumPath =
  process.env.PW_CHROMIUM_PATH ?? (existsSync("/opt/pw-browsers/chromium") ? "/opt/pw-browsers/chromium" : undefined);

// SERAT_BROWSER is derived here, never set by hand. Inherited by the worker processes, which is where the specs print.
process.env.SERAT_BROWSER = chromiumPath ? `executablePath ${chromiumPath}` : "channel chromium (Playwright install, new headless)";

const chromiumUse = {
  ...devices["Desktop Chrome"],
  launchOptions: chromiumPath ? { executablePath: chromiumPath } : { channel: "chromium" as const },
};

export default defineConfig({
  workers: 1, // timed specs must not share the machine with each other or with the unit spec
  forbidOnly: !!process.env.CI,
  testDir: "./tests",
  timeout: 60_000,
  fullyParallel: false, // latency sampling shares one browser instance deliberately
  retries: 0, // a budget harness that retries away a slow run is not a budget harness
  reporter: [["list"], ["json", { outputFile: "out/results.json" }]],
  use: {
    trace: "off", // tracing screenshots load the page being timed
  },
  projects: [
    { name: "unit", testMatch: /measure\.unit\.spec\.ts/, use: chromiumUse },
    { name: "controls", testMatch: /nfr-prf-02-controls\.spec\.ts/, dependencies: ["unit"], use: chromiumUse },
    // The gate: `npx playwright test --project=subject` runs unit, then controls, then subject; a failing
    // control skips the subject tests. `--no-deps` skips the controls; never use it for a gated run (`-g` does not bypass them).
    { name: "subject", testMatch: /nfr-prf-02-subject\.spec\.ts/, dependencies: ["controls"], use: chromiumUse },
    // Task #61: the other §9.3 scenarios and the CRLF / BOM+CRLF fixture variants. Report only; same controls gate.
    { name: "scenarios", testMatch: /nfr-prf-02-scenarios\.spec\.ts/, dependencies: ["controls"], use: chromiumUse },
  ],
});
