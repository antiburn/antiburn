import { defineConfig } from "@playwright/test"
export default defineConfig({
  testDir: ".",
  testMatch: "promotions.spec.ts",
  outputDir: "../../test-results/promotions",
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:4174",
    viewport: { width: 1280, height: 860 },
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "node node_modules/vite/bin/vite.js --config tests/visual/vite.config.ts",
    cwd: new URL("../../", import.meta.url).pathname,
    url: "http://127.0.0.1:4174/tests/visual/",
    timeout: 30_000,
    reuseExistingServer: false,
  },
})
