import type {Page} from '@playwright/test';
import {stubContactsApi} from './contacts-api';

// The shell discovers navigation from the UI contribution endpoint and has no
// development fallback for it, so browser suites that run against the Vite
// server alone answer the shell API with the same fixture data the shell uses.
const workspaces = [
  {id: '018f0000-0000-7000-8000-000000000001', name: 'Northstar Services'},
  {id: '018f0000-0000-7000-8000-000000000002', name: 'Riverview Property Group'},
];

const contributions = {
  navigation: [
    {id: 'contacts', label: 'People', route: '/contacts', icon: 'people', order: 100},
    {id: 'contacts-organisations', label: 'Organisations', route: '/contacts/organisations', icon: 'organisations', order: 110},
  ],
  permissions: [
    'contacts.organisations.custom-fields.read',
    'contacts.organisations.read',
    'contacts.organisations.write',
    'contacts.people.custom-fields.read',
    'contacts.people.read',
    'contacts.people.write',
    'contacts.settings.manage',
  ],
  revision: 'fixture-contacts-enabled',
};

export async function stubShellApi(page: Page) {
  await stubContactsApi(page);
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/workspaces') return route.fulfill({json: {data: workspaces}});
    if (path === '/api/v1/me') return route.fulfill({json: {user: {id: 'local-user', display_name: 'Hannah Carter'}}});
    if (/^\/api\/v1\/workspaces\/[^/]+\/ui\/contributions$/.test(path)) return route.fulfill({json: contributions});
    return route.fallback();
  });
}
