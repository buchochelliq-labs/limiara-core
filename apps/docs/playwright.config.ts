import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  outputDir: "./test-results",
  reporter: process.env.CI ? [["github"], ["html", { open: "never", outputFolder: "playwright-report" }]] : "list",
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  use: {
    baseURL: "http://127.0.0.1:4174",
    browserName: "chromium",
    trace: "retain-on-failure"
  },
  projects: [
    { name: "desktop", use: { viewport: { width: 1280, height: 800 } } },
    { name: "mobile-reflow", use: { viewport: { width: 320, height: 720 } } }
  ],
  webServer: {
    command: "node node_modules/vitepress/bin/vitepress.js preview . --host 127.0.0.1 --port 4174",
    cwd: import.meta.dirname,
    url: "http://127.0.0.1:4174",
    reuseExistingServer: false,
    timeout: 30000
  }
});
