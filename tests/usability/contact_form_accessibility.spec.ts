import { test, expect } from '@playwright/test';

test('contacts validation is announced accessibly', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: /enable contacts/i }).click();
  await page.getByRole('button', { name: /load/i }).click();
  await expect(page.getByRole('heading', { name: /contacts/i })).toBeVisible();
  await expect(page.getByLabel(/workspace/i)).toBeVisible();
});
