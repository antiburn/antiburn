import { defineConfig } from "@playwright/test"

export default defineConfig({
  testDir: "./tests/visual",
  outputDir: "./test-results/visual",
  fullyParallel: false,
  reporter: [["list"], ["html", { outputFolder: "playwright-report/visual", open: "never" }]],
  use: {
    baseURL: "http://127.0.0.1:4174",
    viewport: { width: 1280, height: 860 },
    colorScheme: "light",
    deviceScaleFactor: 1,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "pnpm exec vite --config tests/visual/vite.config.ts",
    url: "http://127.0.0.1:4174/tests/visual/",
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
})
