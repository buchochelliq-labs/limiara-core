import {expect, test, type Page} from '@playwright/test';

const slug = 'S7m_t10aLmN3vW5uQ8zC2bD9fH4jK6sX';
const document = {
  page: {
    page_id: 'platform.forms.contact',
    definition_version: 1,
    selected_variant: null,
    source_versions: {'platform.forms': '1.0.0'},
    publication_versions: [1],
    resource: 'platform.contacts.person',
    data_sources: [],
    actions: [
      {id: 'next', kind: 'navigate', target: 'form.next', parameters: {}},
      {id: 'previous', kind: 'navigate', target: 'form.previous', parameters: {}},
      {id: 'submit-contact', kind: 'resource-operation', target: 'platform.contacts.person.create', parameters: {}},
    ],
    root: {
      id: 'contact-page', component: 'platform.ui.page', props: {title: 'Contact us', description: 'Tell us how we can help.'}, children: [{
        id: 'contact-form', component: 'platform.ui.form', props: {label: 'Contact form'}, children: [{
          id: 'contact-steps', component: 'platform.ui.wizard', props: {label: 'Contact form steps'}, children: [
            {id: 'identity-step', component: 'platform.ui.section', props: {title: 'Your details'}, children: [
              {id: 'email', component: 'platform.ui.field.text', props: {field: 'email'}},
              {id: 'identity-actions', component: 'platform.ui.action-bar', props: {label: 'Step actions'}, children: [
                {id: 'next-button', component: 'platform.ui.action', props: {action: 'next', label: 'Continue', variant: 'primary'}},
              ]},
            ]},
            {id: 'message-step', component: 'platform.ui.section', props: {title: 'Your message'}, children: [
              {id: 'message', component: 'platform.ui.field.text', props: {field: 'message'}},
              {id: 'message-actions', component: 'platform.ui.action-bar', props: {label: 'Step actions'}, children: [
                {id: 'previous-button', component: 'platform.ui.action', props: {action: 'previous', label: 'Back', variant: 'quiet'}},
                {id: 'submit-button', component: 'platform.ui.action', props: {label: 'Send response', variant: 'primary', submit: true}},
              ]},
            ]},
          ],
        }],
      }],
    },
    content_hash: 'external-fixture-hash',
  },
  resources: [{
    id: 'platform.contacts.person',
    definition_version: 1,
    owner_plugin: 'platform.contacts',
    label: 'Contact response',
    fields: [
      {api_name: 'email', label: 'Email address', description: 'We will use this only to reply.', field_type: 'email', source: 'standard', readable: true, writable: true, required: true, classification: 'personal', options: [], submit_when_hidden: false},
      {api_name: 'message', label: 'Message', field_type: 'text', source: 'standard', readable: true, writable: true, required: true, classification: 'personal', options: [], submit_when_hidden: false},
    ],
    relationships: [],
    operations: [{id: 'create', kind: 'create', concurrency: 'none', hosts: ['public-form']}],
  }],
};

async function mockExternalApi(page: Page) {
  await page.route('**/api/v1/external/forms/**', async route => {
    if (route.request().method() === 'POST') {
      const body = route.request().postDataJSON() as {payload: Record<string, unknown>};
      expect(body).toEqual({payload: {email: 'person@example.test', message: 'Please call me.'}});
      expect(route.request().headers()['idempotency-key']).toMatch(/^[a-z0-9]{16,}$/i);
      expect(JSON.stringify(body)).not.toContain('tenant');
      await route.fulfill({status: 202, contentType: 'application/json', body: JSON.stringify({submission_id: 'submission-browser-1', correlation_id: 'correlation-1', replayed: false})});
      return;
    }
    await route.fulfill({status: 200, contentType: 'application/json', body: JSON.stringify(document)});
  });
}

test('public metadata form is keyboard-operable, localized, themed, and completes accessibly', async ({page}) => {
  await mockExternalApi(page);
  await page.goto(`/forms/${slug}`);
  await expect(page.getByRole('heading', {level: 1, name: 'Contact us'})).toBeVisible();
  await expect(page.getByRole('main')).toBeVisible();
  await expect(page).toHaveScreenshot('external-public-form.png', {fullPage: true});
  await page.getByLabel('Email address').fill('person@example.test');
  await page.getByRole('button', {name: 'Continue'}).click();
  await expect(page.getByRole('group', {name: 'Step 2 of 2'})).toBeFocused();
  await page.getByLabel('Message', {exact: true}).fill('Please call me.');
  await page.getByRole('button', {name: 'Send response'}).click();
  await expect(page.getByRole('heading', {level: 1, name: 'Thank you'})).toBeVisible();
  await expect(page.getByRole('status')).toContainText('submission-browser-1');

  await page.getByLabel('Colour scheme').selectOption('dark');
  await expect(page.locator('.external-host')).toHaveAttribute('data-theme', 'dark');
  await page.getByLabel('Language').selectOption('fr-FR');
  await expect(page.locator('html')).toHaveAttribute('lang', 'fr-FR');
});

test('external host reflows at 320px and preserves forced-colour semantics', async ({page}) => {
  await mockExternalApi(page);
  await page.setViewportSize({width: 320, height: 720});
  await page.emulateMedia({forcedColors: 'active', reducedMotion: 'reduce'});
  await page.goto(`/forms/${slug}?theme=contrast`);
  await expect(page.getByRole('heading', {level: 1, name: 'Contact us'})).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  expect(await page.evaluate(() => matchMedia('(forced-colors: active)').matches)).toBe(true);
  expect(await page.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches)).toBe(true);
  await expect(page.getByRole('link', {name: 'Skip to content'})).toBeAttached();
});

test('malformed and unknown external links use the same standalone unavailable experience', async ({page}) => {
  await page.route('**/api/v1/external/forms/**', route => route.fulfill({status: 404, contentType: 'application/problem+json', body: '{}'}));
  await page.goto('/forms/short');
  const malformed = await page.getByRole('alert').textContent();
  await page.goto(`/forms/${slug}`);
  const unknown = await page.getByRole('alert').textContent();
  expect(malformed).toBe(unknown);
  await expect(page.getByRole('navigation', {name: 'Primary navigation'})).toHaveCount(0);
});
