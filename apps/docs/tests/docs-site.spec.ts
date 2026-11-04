import { expect, test } from "@playwright/test";

async function expectNoHorizontalOverflow(page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
}

test("home exposes keyboard landmarks and reflows", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Limiara");
  await expect(page.getByRole("main")).toBeVisible();
  await expectNoHorizontalOverflow(page);

  await page.keyboard.press("Tab");
  const skipLink = page.getByRole("link", { name: "Skip to content" });
  await expect(skipLink).toBeFocused();

  if (testInfo.project.name === "mobile-reflow") {
    await expect(page.getByRole("navigation", { name: "Main Navigation" })).toBeHidden();
    const menu = page.getByRole("button", { name: "mobile navigation" });
    await menu.click();
    await expect(menu).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByRole("button", { name: "Product", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Build", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Operate", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Contribute", exact: true })).toBeVisible();
  } else {
    await expect(page.getByRole("navigation", { name: "Main Navigation" })).toBeVisible();
  }
});

test("local search is labelled and returns repository content", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: /Search/ }).click();
  const search = page.getByRole("searchbox");
  await search.fill("metadata");
  await expect(page.getByRole("listbox")).toBeVisible();
  await expect(page.getByRole("option", { name: "UI metadata conformance validation", exact: true })).toBeVisible();
});

test("theme and reduced-motion preferences remain operable", async ({ page }, testInfo) => {
  await page.emulateMedia({ reducedMotion: "reduce", colorScheme: "light" });
  await page.goto("/");
  expect(await page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches)).toBe(true);
  if (testInfo.project.name === "mobile-reflow") {
    await page.getByRole("button", { name: "mobile navigation" }).click();
  }
  const switcher = page.getByRole("switch", { name: "Switch to dark theme" });
  await switcher.click();
  await expect(page.locator("html")).toHaveClass(/dark/);
  const duration = await page.locator(".VPNavBar").evaluate((element) => getComputedStyle(element).transitionDuration);
  expect(Number.parseFloat(duration)).toBeLessThanOrEqual(0.01);
});

test("API reference documents only the approved public operation", async ({ page }) => {
  await page.goto("/api/reference");
  await expect(page.getByRole("main")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("API reference proof");
  await expect(page.getByText("/health/live", { exact: true })).toBeVisible();
  await expect(page.getByText("getLiveness", { exact: true })).toBeVisible();
  await expectNoHorizontalOverflow(page);
});

test("unknown routes render a navigable 404 page", async ({ page }) => {
  const response = await page.goto("/definitely-not-a-page");
  expect(response?.status()).toBe(404);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await expect(page.getByRole("link", { name: /home/i })).toBeVisible();
});

test("governance status and owner are visible on public pages", async ({ page }) => {
  await page.goto("/architecture/odata");
  const governance = page.getByRole("complementary", { name: "Document status" });
  await expect(governance).toBeVisible();
  await expect(governance).toContainText("Planned");
  await expect(governance).toContainText("API and event platform");
  await expect(governance.getByRole("link", { name: "How status and ownership work" })).toBeVisible();
});
