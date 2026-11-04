// Browser layer of the #190 lifecycle acceptance journey.
//
//   node tests/lifecycle-acceptance/shell.mjs <enabled|disabled>
//
// Serves the production web build (apps/web/dist) same-origin with the real
// API (API_BASE), signs in with the seeded session cookie (SESSION_TOKEN), and
// checks that the shell's navigation and a direct deep link follow the
// workspace's Contacts lifecycle state. While enabled it also drives the
// people workflows end to end against the API: create, reload the deep link,
// edit, and archive. Navigation comes only from the API's UI contribution
// endpoint and records only from the Contacts API; nothing here stubs them.
import {createServer, request as httpRequest} from 'node:http';
import {readFile, stat} from 'node:fs/promises';
import {extname, join, normalize} from 'node:path';
import {fileURLToPath} from 'node:url';
import {chromium} from '@playwright/test';

const phase = process.argv[2];
if (phase !== 'enabled' && phase !== 'disabled') {
  console.error('usage: shell.mjs <enabled|disabled>');
  process.exit(2);
}
const apiBase = new URL(process.env.API_BASE ?? 'http://127.0.0.1:18080');
const sessionToken = process.env.SESSION_TOKEN;
const cookieName = process.env.SESSION_COOKIE ?? 'platform_session';
if (!sessionToken) {
  console.error('SESSION_TOKEN is required');
  process.exit(2);
}

const dist = fileURLToPath(new URL('../../apps/web/dist/', import.meta.url));
const types = {'.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.png': 'image/png'};

const server = createServer(async (incoming, outgoing) => {
  const url = new URL(incoming.url ?? '/', 'http://shell.local');
  if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/auth/') || url.pathname.startsWith('/health/')) {
    const upstream = httpRequest({
      host: apiBase.hostname,
      port: apiBase.port,
      method: incoming.method,
      path: url.pathname + url.search,
      headers: {...incoming.headers, host: apiBase.host},
    }, response => {
      outgoing.writeHead(response.statusCode ?? 502, response.headers);
      response.pipe(outgoing);
    });
    upstream.on('error', () => { outgoing.writeHead(502); outgoing.end(); });
    incoming.pipe(upstream);
    return;
  }
  // Static assets, with the SPA fallback that deep links rely on.
  let file = normalize(join(dist, decodeURIComponent(url.pathname)));
  if (!file.startsWith(dist)) { outgoing.writeHead(400); outgoing.end(); return; }
  try {
    if (!(await stat(file)).isFile()) throw new Error('not a file');
  } catch {
    file = join(dist, 'index.html');
  }
  outgoing.writeHead(200, {'content-type': types[extname(file)] ?? 'application/octet-stream'});
  outgoing.end(await readFile(file));
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

// The person the journey script created before this phase marks the rendered directory.
const directory = page => page.getByText('Ada Lovelace').first();

const visible = async (locator, timeout = 15_000) => {
  await locator.waitFor({timeout}).catch(() => {});
  return locator.isVisible();
};

const failures = [];
const check = (condition, message) => { if (!condition) failures.push(message); };

const browser = await chromium.launch();
try {
  const context = await browser.newContext();
  await context.addCookies([{name: cookieName, value: sessionToken, url: origin, httpOnly: true, sameSite: 'Lax'}]);
  const page = await context.newPage();
  const contributions = [];
  const apiFailures = [];
  page.on('response', response => {
    if (response.url().includes('/ui/contributions')) contributions.push(response.status());
    if (response.url().includes('/api/') && response.status() >= 400) {
      response.text().then(body => apiFailures.push(`${response.request().method()} ${new URL(response.url()).pathname} -> ${response.status()} ${body.slice(0, 300)}`)).catch(() => {});
    }
  });
  page.on('pageerror', error => apiFailures.push(`page error: ${error.message}`));
  // Diagnostics for a failed step: the page's main text and every failed API call.
  const diagnose = async step => {
    const main = await page.getByRole('main').innerText().catch(() => '(no main landmark)');
    failures.push(`${step}\n  main: ${main.replace(/\s+/g, ' ').slice(0, 600)}\n  api: ${apiFailures.join('; ') || 'no failed API responses'}`);
  };

  await page.goto(`${origin}/`);
  const navigation = page.getByRole('navigation', {name: 'Primary navigation'}).first();
  await page.getByRole('heading', {name: 'Workspace overview'}).waitFor({timeout: 15_000}).catch(() => {});
  const contactsLink = navigation.getByRole('link', {name: 'People'});

  if (phase === 'enabled') {
    await contactsLink.waitFor({timeout: 15_000}).catch(() => {});
    check(await contactsLink.isVisible(), 'enabled: primary navigation does not offer People');
    await page.goto(`${origin}/contacts`);
    await directory(page).waitFor({timeout: 15_000}).catch(() => {});
    check(await directory(page).isVisible(), 'enabled: the /contacts deep link does not render the people directory');
    check(!(await page.getByText('Route unavailable').isVisible()), 'enabled: the /contacts deep link is reported unavailable');

    // Create → reload → edit → archive through the production build and the real API.
    try {
    const stamp = Date.now().toString(36);
    const name = `Browser Person ${stamp}`;
    const email = `browser.${stamp}@example.test`;
    await page.goto(`${origin}/contacts/people/new`);
    await page.getByLabel('Full name').fill(name);
    await page.getByLabel('Email address').fill(email);
    await page.getByRole('button', {name: 'Save person'}).click();
    check(await visible(page.getByText('Person added')), 'enabled: creating a person through the form did not succeed');
    await page.getByRole('button', {name: 'Open person'}).click();
    check(await visible(page.getByRole('heading', {level: 1, name})), 'enabled: the created person did not open');
    await page.reload();
    check(await visible(page.getByRole('heading', {level: 1, name})), 'enabled: reloading the person deep link did not read the record from the API');
    check(await visible(page.getByText(email).first()), 'enabled: the reloaded person does not show its saved email');
    await page.getByRole('button', {name: 'Edit person'}).click();
    check(await visible(page.getByText('editing version 1')), 'enabled: the edit form does not show the saved version');
    await page.getByLabel('Full name').fill(`${name} Updated`);
    await page.getByRole('button', {name: 'Save changes'}).click();
    check(await visible(page.getByText('Changes saved')), 'enabled: editing a person through the form did not succeed');
    await page.getByRole('button', {name: 'Return to person'}).click();
    check(await visible(page.getByRole('heading', {level: 1, name: `${name} Updated`})), 'enabled: the edited name is not shown on the record');
    page.once('dialog', dialog => dialog.accept());
    await page.getByRole('button', {name: 'Archive person'}).click();
    check(await visible(page.getByText('Archived person')), 'enabled: archiving a person did not succeed');
    check((await page.getByRole('button', {name: 'Archive person'}).count()) === 0, 'enabled: an archived person still offers archive');
    await page.goto(`${origin}/contacts?q=${encodeURIComponent(name)}`);
    check(await visible(page.getByText('No people match this search')), 'enabled: the archived person still appears in the active directory');
    } catch (error) {
      await diagnose(`enabled: the people workflow journey failed: ${error.message.split('\n')[0]}`);
    }
  } else {
    // The workspace's own Reports entry (ADR 0027) stays, so the shell shows
    // either it alone or, without the object engine, its empty state.
    const settled = page.getByText('No features enabled').or(navigation.getByRole('link', {name: 'Reports'}));
    await settled.first().waitFor({timeout: 15_000}).catch(() => {});
    check(await settled.first().isVisible(), 'disabled: the shell did not settle on the remaining workspace features');
    check((await contactsLink.count()) === 0, 'disabled: primary navigation still offers People');
    // With no feature enabled the shell shows its empty state on every route;
    // with other features enabled it would show "Route unavailable". Either
    // way the Contacts directory must not render.
    await page.goto(`${origin}/contacts`);
    const concealed = page.getByText('No features enabled').or(page.getByText('Route unavailable'));
    await concealed.first().waitFor({timeout: 15_000}).catch(() => {});
    check(await concealed.first().isVisible(), 'disabled: the /contacts deep link is not reported unavailable');
    check((await directory(page).count()) === 0, 'disabled: the /contacts deep link renders the people directory');
    check(!(await page.getByRole('link', {name: 'People'}).count()), 'disabled: the deep link still renders Contacts navigation');
  }
  check(contributions.length > 0 && contributions.every(status => status === 200), `contribution requests did not all succeed: ${contributions.join(',')}`);
} finally {
  await browser.close();
  server.close();
}

if (failures.length) {
  for (const failure of failures) console.error(`FAIL: ${failure}`);
  process.exit(1);
}
console.log(`shell ${phase}: navigation, deep links, and people workflows follow the lifecycle state`);
