import {defineConfig} from '@playwright/test';

export default defineConfig({
  testDir: './tests/contacts-metadata',
  outputDir: './test-results/contacts-metadata',
  snapshotPathTemplate: '{testDir}/{testFilePath}-snapshots/{arg}-{projectName}{ext}',
  reporter: process.env.CI ? [['github'], ['html', {open: 'never', outputFolder: 'playwright-report/contacts-metadata'}]] : 'list',
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  expect: {toHaveScreenshot: {animations: 'disabled', maxDiffPixelRatio: 0.015}},
  use: {
    baseURL: 'http://127.0.0.1:4176',
    browserName: 'chromium',
    colorScheme: 'light',
    trace: 'retain-on-failure',
  },
  projects: [
    {name: 'desktop', use: {viewport: {width: 1440, height: 900}}},
    {name: 'tablet', use: {viewport: {width: 834, height: 1194}, hasTouch: true}},
    {name: 'mobile', use: {viewport: {width: 390, height: 844}, hasTouch: true}},
  ],
});
