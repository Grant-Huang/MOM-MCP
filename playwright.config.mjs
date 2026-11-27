import { defineConfig } from "@playwright/test";

const baseURL = "http://127.0.0.1:4173";
export default defineConfig({
  testDir: "./tests/browser",
  outputDir: "test-results/playwright",
  reporter: [["list"], ["json", { outputFile: "test-results/browser.json" }]],
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    launchOptions: { executablePath: process.env.MOM_CHROMIUM || undefined },
  },
  webServer: {
    command: process.env.MOM_ACCEPTANCE_BINARY
      ? '"$MOM_ACCEPTANCE_BINARY" web --control-tools --db "$MOM_TEST_DB"'
      : 'cargo run --locked -- web --control-tools --db "$MOM_TEST_DB"',
    url: baseURL,
    timeout: 120_000,
  },
});
