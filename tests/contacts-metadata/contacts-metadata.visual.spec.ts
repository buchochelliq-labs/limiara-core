import {expect, test} from '@playwright/test';
import {stubShellApi} from './shell-api';

test.beforeEach(async ({page}) => {
  await stubShellApi(page);
});

const pages = [
  {name: 'people-directory', url: '/contacts?selected=maya-patel', heading: 'People'},
  {name: 'person-detail', url: '/contacts/people/maya-patel', heading: 'Maya Patel'},
  {name: 'person-edit-form', url: '/contacts/people/maya-patel/edit', heading: 'Edit Maya Patel'},
  {name: 'organisation-directory', url: '/contacts/organisations?selected=northstar', heading: 'Organisations'},
  {name: 'organisation-detail', url: '/contacts/organisations/northstar', heading: 'Northstar Services'},
] as const;

async function expectNoHorizontalOverflow(page: import('@playwright/test').Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
}

for (const view of pages) {
  test(`${view.name} has an accessible responsive visual baseline`, async ({page}, testInfo) => {
    await page.goto(view.url);
    await expect(page.getByRole('heading', {level: 1, name: view.heading})).toBeVisible();
    await expect(page.getByRole('main')).toBeVisible();
    await expectNoHorizontalOverflow(page);
    await expect(page).toHaveScreenshot(`${view.name}.png`, {fullPage: true});

    if (testInfo.project.name === 'mobile') {
      await expect(page.getByRole('button', {name: 'Open navigation'})).toBeVisible();
      await expect(page.getByRole('navigation', {name: 'Primary navigation'}).first()).toBeHidden();
    }
  });
}

test('workspace home lists enabled features and the launcher opens, filters, and closes', async ({page}, testInfo) => {
  await page.clock.setFixedTime(new Date('2026-10-11T09:30:00Z'));
  await page.goto('/');
  await expect(page.getByRole('heading', {level: 1, name: 'Good morning, Hannah'})).toBeVisible();
  const apps = page.getByRole('region', {name: 'Your features'});
  await expect(apps.getByRole('link', {name: /People/})).toBeVisible();
  await expect(apps.getByRole('link', {name: /Organisations/})).toBeVisible();
  await expectNoHorizontalOverflow(page);
  await expect(page).toHaveScreenshot('workspace-home.png', {fullPage: true});

  if (testInfo.project.name === 'mobile') return;
  const launcher = page.getByRole('button', {name: 'Feature launcher'});
  await launcher.click();
  const panel = page.getByRole('dialog', {name: 'Feature launcher'});
  await expect(panel).toBeVisible();
  await expect(panel.getByRole('link', {name: /People/})).toBeVisible();
  await panel.getByLabel('Find a feature').fill('org');
  await expect(panel.getByRole('link', {name: /People/})).toHaveCount(0);
  await expect(panel.getByRole('link', {name: /Organisations/})).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(panel).toHaveCount(0);
  await expect(launcher).toBeFocused();
  await launcher.click();
  await panel.getByRole('link', {name: /Organisations/}).click();
  await expect(page.getByRole('heading', {level: 1, name: 'Organisations'})).toBeVisible();
  expect(new URL(page.url()).pathname).toBe('/contacts/organisations');
});

test('people can be added, edited, and archived through the Contacts API contract', async ({page}) => {
  await page.goto('/contacts');
  await expect(page.getByRole('heading', {level: 1, name: 'People'})).toBeVisible();
  await expect(page.getByText('Showing 8', {exact: true})).toBeVisible();
  await page.getByRole('button', {name: 'Add person'}).first().click();
  await expect(page.getByRole('heading', {level: 1, name: 'Add person'})).toBeVisible();
  await page.getByLabel('Full name').fill('Journey Person');
  await page.getByLabel('Email address').fill('maya@northstar.example');
  await page.getByRole('button', {name: 'Save person'}).click();
  await expect(page.getByText('A person with a similar name and email may already exist')).toBeVisible();
  await page.getByLabel('Email address').fill('journey@example.test');
  await page.getByLabel('Organisation').selectOption('northstar');
  await page.getByRole('button', {name: 'Save person'}).click();
  await expect(page.getByText('Person added')).toBeVisible();
  await page.getByRole('button', {name: 'Open person'}).click();
  await expect(page.getByRole('heading', {level: 1, name: 'Journey Person'})).toBeVisible();
  await expect(page.getByText('journey@example.test').first()).toBeVisible();
  await expect(page.getByRole('main').getByText('Northstar Services').first()).toBeVisible();

  await page.getByRole('button', {name: 'Edit person'}).click();
  await expect(page.getByRole('heading', {level: 1, name: 'Edit Journey Person'})).toBeVisible();
  await expect(page.getByText('editing version 1')).toBeVisible();
  await page.getByLabel('Full name').fill('Journey Person Updated');
  await page.getByRole('button', {name: 'Save changes'}).click();
  await expect(page.getByText('Changes saved')).toBeVisible();
  await page.getByRole('button', {name: 'Return to person'}).click();
  await expect(page.getByRole('heading', {level: 1, name: 'Journey Person Updated'})).toBeVisible();

  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', {name: 'Archive person'}).click();
  await expect(page.getByText('Archived person')).toBeVisible();
  await expect(page.getByRole('button', {name: 'Archive person'})).toHaveCount(0);
  await expect(page.getByRole('button', {name: 'Restore person'})).toHaveCount(0);

  await page.getByRole('button', {name: 'Back to people'}).click();
  await expect(page.getByText('Showing 8', {exact: true})).toBeVisible();
  await expect(page.getByText('Journey Person')).toHaveCount(0);
});

test('a version conflict shows the latest saved values instead of overwriting them', async ({page}) => {
  await page.goto('/contacts/people/maya-patel/edit');
  await expect(page.getByText('editing version 7')).toBeVisible();
  // Another session saves first.
  await page.evaluate(() => fetch('/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/contacts/people/maya-patel', {method: 'PUT', headers: {'content-type': 'application/json'}, body: JSON.stringify({expected_version: 7, full_name: 'Maya Patel', email: 'maya.patel@northstar.example', telephone: null, organisation_id: 'northstar', tags: [], custom_fields: {}})}));
  await page.getByLabel('Full name').fill('Maya P.');
  await page.getByRole('button', {name: 'Save changes'}).click();
  await expect(page.getByText(/changed since|newer|latest/i).first()).toBeVisible();
});

test('collection keyboard movement and compact preview remain operable', async ({page}) => {
  await page.goto('/contacts/organisations');
  const rows = page.locator('.meta-collection__select');
  await expect(rows.first().locator('dt')).toHaveText(['Sector', 'Phone', 'Account status', 'Updated']);
  await rows.first().focus();
  await page.keyboard.press('ArrowDown');
  await expect(rows.nth(1)).toBeFocused();
  await page.keyboard.press('End');
  await expect(rows.last()).toBeFocused();
  await rows.last().press('Enter');
  await expect(page.getByRole('complementary', {name: /preview/i})).toBeVisible();
});

test('forced colours and reduced motion preserve Contacts content', async ({page}) => {
  await page.emulateMedia({forcedColors: 'active', reducedMotion: 'reduce'});
  await page.goto('/contacts/people/maya-patel');
  expect(await page.evaluate(() => matchMedia('(forced-colors: active)').matches)).toBe(true);
  expect(await page.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches)).toBe(true);
  await expect(page.getByRole('heading', {level: 1, name: 'Maya Patel'})).toBeVisible();
  await expect(page.getByRole('button', {name: 'Edit person'})).toBeVisible();
  await expectNoHorizontalOverflow(page);
});

test('320px reflow preserves detail actions and content without horizontal scrolling', async ({page}) => {
  await page.setViewportSize({width: 320, height: 720});
  await page.goto('/contacts/organisations/northstar');
  await expect(page.getByRole('heading', {level: 1, name: 'Northstar Services'})).toBeVisible();
  await expect(page.getByRole('button', {name: 'Edit organisation'})).toBeVisible();
  await expect(page.getByRole('button', {name: 'Archive organisation'})).toBeVisible();
  await expectNoHorizontalOverflow(page);
});

test('200% desktop reflow keeps primary journeys and mobile navigation usable', async ({page}) => {
  // 720 CSS pixels is the effective layout width of a 1440px desktop viewport
  // at 200% browser zoom. Testing the CSS-pixel viewport avoids relying on a
  // browser-specific zoom shortcut while exercising the same reflow boundary.
  await page.setViewportSize({width: 720, height: 450});
  await page.goto('/contacts/organisations?selected=northstar');
  await expect(page.getByRole('heading', {level: 1, name: 'Organisations'})).toBeVisible();
  await expect(page.getByRole('button', {name: 'Add organisation'})).toBeVisible();
  await expect(page.getByRole('button', {name: 'Open navigation'})).toBeVisible();
  await expectNoHorizontalOverflow(page);

  const menuButton = page.getByRole('button', {name: 'Open navigation'});
  await menuButton.click();
  const drawer = page.getByRole('dialog', {name: 'Navigation menu'});
  const firstDrawerLink = drawer.getByRole('link', {name: 'Limiara', exact: true});
  const settings = drawer.getByRole('link', {name: 'Settings'});
  await expect(firstDrawerLink).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(settings).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(firstDrawerLink).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menuButton).toBeFocused();

  await menuButton.click();
  await drawer.getByRole('link', {name: 'Settings'}).click();
  await expect(drawer).toHaveCount(0);
  expect(new URL(page.url()).pathname).toBe('/settings');
});

test('long form values survive phone, tablet, and landscape viewport changes', async ({page}) => {
  const longName = 'Alexandria Catherine Montgomery-Winterbottom';
  const longEmail = 'alexandria.montgomery-winterbottom@international-field-services.example';

  await page.setViewportSize({width: 390, height: 844});
  await page.goto('/contacts/people/maya-patel/edit');
  const name = page.getByLabel('Full name');
  const email = page.getByLabel('Email address');
  await name.fill(longName);
  await email.fill(longEmail);
  await expectNoHorizontalOverflow(page);

  await page.setViewportSize({width: 834, height: 1194});
  await expect(name).toHaveValue(longName);
  await expect(email).toHaveValue(longEmail);
  await expectNoHorizontalOverflow(page);

  await page.setViewportSize({width: 844, height: 390});
  await expect(name).toHaveValue(longName);
  await expect(email).toHaveValue(longEmail);
  await expect(page.locator('[data-metadata-node="form-actions"]')).toHaveCSS('position', 'static');
  const save = page.getByRole('button', {name: 'Save changes'});
  const cancel = page.getByRole('button', {name: 'Cancel'});
  await save.scrollIntoViewIfNeeded();
  await expect(save).toBeInViewport();
  await expect(cancel).toBeInViewport();
  await expectNoHorizontalOverflow(page);
});

test('compact empty, denied, and error states preserve messages and available actions', async ({page}) => {
  await page.setViewportSize({width: 390, height: 844});
  await page.goto('/contacts?people-state=empty');
  const emptyState = page.locator('[data-metadata-node="people-empty"]');
  await expect(emptyState.getByRole('heading', {name: 'Add your first person'})).toBeVisible();
  await expect(emptyState.getByRole('button', {name: 'Add person'})).toBeVisible();
  await expectNoHorizontalOverflow(page);

  await page.goto('/contacts?people-state=denied');
  const deniedState = page.locator('[data-metadata-node="people-denied"]');
  await expect(deniedState.getByRole('status')).toContainText('Content unavailable');
  await expect(deniedState.getByRole('status')).toContainText('You do not have access to this content.');
  await expectNoHorizontalOverflow(page);

  await page.goto('/contacts?people-state=error');
  const errorState = page.locator('[data-metadata-node="people-error"]');
  await expect(errorState.getByRole('alert')).toContainText('People could not be loaded');
  await expect(errorState.getByRole('alert')).toContainText('Check your connection and try again. Your search has been preserved.');
  await expectNoHorizontalOverflow(page);
});

test('virtual-keyboard height keeps form validation and actions reachable', async ({page}) => {
  await page.setViewportSize({width: 390, height: 400});
  await page.goto('/contacts/people/new');
  const fullName = page.getByLabel('Full name');
  const email = page.getByLabel('Email address');
  const save = page.getByRole('button', {name: 'Save person'});
  await email.fill('invalid');
  await save.click();
  await expect(page.getByText('Full name is required.')).toBeVisible();
  await expect(fullName).toBeFocused();

  await fullName.fill('A Test Person');
  await save.click();
  await expect(page.getByText('Enter a valid email address.')).toBeInViewport();
  await expect(email).toBeFocused();
  await expect(email).toBeInViewport();
  await expect(page.locator('[data-metadata-node="form-actions"]')).toHaveCSS('position', 'static');
  await save.scrollIntoViewIfNeeded();
  await expect(save).toBeInViewport();
  await expect(page.getByRole('button', {name: 'Cancel'})).toBeInViewport();
  await expectNoHorizontalOverflow(page);
});

test('mobile controls meet Limiara and WCAG target-size rules', async ({page}) => {
  await page.setViewportSize({width: 390, height: 844});
  for (const route of ['/contacts?selected=maya-patel', '/contacts/people/maya-patel', '/contacts/people/maya-patel/edit']) {
    await page.goto(route);
    if (route === '/contacts/people/maya-patel') {
      const shortCallerSafeLink = page.locator('.meta-display-fields a').first();
      await expect(shortCallerSafeLink).toBeVisible();
      await shortCallerSafeLink.evaluate(element => { element.textContent = 'x'; });
    }
    const undersized = await page.locator('a[href], button, input, select').evaluateAll(elements => elements.flatMap(element => {
      const target = element.matches('.global-search input') ? element.closest('.global-search') ?? element : element;
      const style = getComputedStyle(target);
      const rect = target.getBoundingClientRect();
      if (style.display === 'none' || style.visibility === 'hidden' || rect.width === 0 || rect.height === 0 || rect.bottom <= 0) return [];
      const documentedCompactSecondaryException = element.matches('button.ui-button--small.ui-button--secondary');
      const minimum = documentedCompactSecondaryException ? 36 : 44;
      return rect.width < minimum || rect.height < minimum ? [{name: element.getAttribute('aria-label') ?? element.textContent?.trim() ?? element.tagName, width: rect.width, height: rect.height, minimum}] : [];
    }));
    expect(undersized).toEqual([]);
  }

  await page.goto('/contacts');
  await page.getByRole('button', {name: 'Open navigation'}).click();
  const drawerTargets = await page.getByRole('dialog', {name: 'Navigation menu'}).locator('a[href], button, input, select').evaluateAll(elements => elements.flatMap(element => {
    const target = element.matches('.global-search input') ? element.closest('.global-search') ?? element : element;
    const rect = target.getBoundingClientRect();
    const documentedCompactSecondaryException = element.matches('button.ui-button--small.ui-button--secondary');
    const minimum = documentedCompactSecondaryException ? 36 : 44;
    return rect.width < minimum || rect.height < minimum ? [{name: element.getAttribute('aria-label') ?? element.textContent?.trim() ?? element.tagName, width: rect.width, height: rect.height, minimum}] : [];
  }));
  expect(drawerTargets).toEqual([]);
});

test('route, selection, and drawer state remain coherent across orientation changes', async ({page}) => {
  await page.setViewportSize({width: 390, height: 844});
  await page.goto('/contacts?q=maya&selected=maya-patel');
  await expect(page.getByRole('complementary', {name: 'Maya Patel preview'})).toBeVisible();
  expect(new URL(page.url()).pathname).toBe('/contacts');
  expect(new URL(page.url()).searchParams.get('q')).toBe('maya');
  await page.getByRole('button', {name: 'Open navigation'}).click();
  const drawer = page.getByRole('dialog', {name: 'Navigation menu'});
  await expect(drawer).toBeVisible();
  await expect(drawer.getByRole('link', {name: 'Settings'})).toBeVisible();

  await page.setViewportSize({width: 844, height: 390});
  await expect(drawer).toBeHidden();
  expect(new URL(page.url()).pathname).toBe('/contacts');
  expect(new URL(page.url()).searchParams.get('q')).toBe('maya');
  expect(new URL(page.url()).searchParams.get('selected')).toBe('maya-patel');

  await page.setViewportSize({width: 390, height: 844});
  await expect(page.getByRole('dialog', {name: 'Navigation menu'})).toHaveCount(0);
  await expect(page.getByRole('complementary', {name: 'Maya Patel preview'})).toBeVisible();
  expect(new URL(page.url()).searchParams.get('q')).toBe('maya');
  expect(new URL(page.url()).searchParams.get('selected')).toBe('maya-patel');
  await expectNoHorizontalOverflow(page);
});

test('translated-length primary actions wrap without clipping or page overflow', async ({page}) => {
  await page.setViewportSize({width: 390, height: 844});
  await page.goto('/contacts');
  const addPerson = page.getByRole('button', {name: 'Add person'});
  await addPerson.evaluate(element => {
    const label = element.querySelector('span');
    if (label) label.textContent = 'Add a person with complete workspace details';
  });
  await expect(page.getByRole('button', {name: 'Add a person with complete workspace details'})).toBeVisible();
  await expectNoHorizontalOverflow(page);
});

test('representative text and primary actions meet WCAG AA contrast', async ({page}) => {
  await page.goto('/contacts');
  // Navigation is discovered asynchronously; measure only rendered content.
  await expect(page.locator('[data-metadata-node="add-person-button"]')).toBeVisible();
  const ratios = await page.evaluate(() => {
    const luminance = (colour: string) => {
      const channels = colour.match(/[\d.]+/g)?.slice(0, 3).map(Number) ?? [0, 0, 0];
      const values = channels.map(value => {
        const channel = value / 255;
        return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * values[0] + 0.7152 * values[1] + 0.0722 * values[2];
    };
    const ratio = (foreground: string, background: string) => {
      const [lighter, darker] = [luminance(foreground), luminance(background)].sort((left, right) => right - left);
      return (lighter + 0.05) / (darker + 0.05);
    };
    const body = getComputedStyle(document.body);
    const primary = getComputedStyle(document.querySelector<HTMLButtonElement>('[data-metadata-node="add-person-button"]')!);
    return [ratio(body.color, body.backgroundColor), ratio(primary.color, primary.backgroundColor)];
  });
  expect(ratios.every(value => value >= 4.5)).toBe(true);
});
