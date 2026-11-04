// Browser layer of the ADR 0015 object engine acceptance journey.
//
//   node tests/objects-acceptance/browser.mjs
//
// Serves the production web build (apps/web/dist) same-origin with the real
// API (API_BASE), signs in with the seeded administrator session
// (SESSION_TOKEN), and drives the object pages end to end: navigation from the
// UI contribution endpoint, the object home, create, edit, delete and restore
// from the recycle bin, then the Object Manager creating an object and a
// field. Nothing here stubs the API. Screenshots go to SCREENSHOT_DIR.
import {createServer, request as httpRequest} from 'node:http';
import {mkdir, readFile, stat} from 'node:fs/promises';
import {extname, join, normalize} from 'node:path';
import {fileURLToPath} from 'node:url';
import {chromium} from '@playwright/test';

const apiBase = new URL(process.env.API_BASE ?? 'http://127.0.0.1:18091');
const sessionToken = process.env.SESSION_TOKEN;
const screenshots = process.env.SCREENSHOT_DIR ?? '';
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

const step = message => console.log(`   - ${message}`);
const shot = async (page, name) => {
  if (!screenshots) return;
  await mkdir(screenshots, {recursive: true});
  await page.screenshot({path: join(screenshots, `${name}.png`), fullPage: true});
};

const browser = await chromium.launch();
let failed = false;
let page;
try {
  const context = await browser.newContext({viewport: {width: 1280, height: 900}});
  await context.addCookies([{name: 'platform_session', value: sessionToken, url: origin, httpOnly: true, sameSite: 'Lax'}]);
  page = await context.newPage();
  const timeout = 15_000;
  page.setDefaultTimeout(timeout);

  step('the rail shows Projects from the UI contribution endpoint');
  await page.goto(`${origin}/`);
  const nav = page.getByRole('navigation', {name: 'Primary navigation'});
  await nav.getByRole('link', {name: 'Projects'}).click();
  await page.getByRole('heading', {level: 1, name: 'Projects'}).waitFor();
  await page.getByRole('link', {name: 'Apollo Prime'}).waitFor();
  if (await page.getByRole('link', {name: 'Clients'}).count()) throw new Error('objects outside navigation must not appear in the rail');
  await shot(page, '01-object-home');

  step('create a record from field metadata');
  await page.getByRole('button', {name: 'New project'}).click();
  await page.getByRole('heading', {level: 1, name: 'New project'}).waitFor();
  await page.getByLabel('Project name').fill('Browser project');
  await page.getByLabel('Code').fill('BR-1');
  await page.getByLabel('Status').selectOption('won');
  await page.getByLabel('Starts on').fill('2026-12-01');
  await page.getByLabel('Client', {exact: true}).selectOption({label: 'Analytical Engines'});
  await shot(page, '02-new-record');
  await page.getByRole('button', {name: 'Save'}).click();
  await page.getByRole('heading', {level: 1, name: 'Browser project'}).waitFor();
  await page.getByText('Won', {exact: true}).waitFor();
  await page.getByRole('link', {name: 'Analytical Engines'}).waitFor();
  await shot(page, '03-record');

  step('a duplicate unique value is explained, not lost');
  await page.goto(`${origin}/o/custom:project/new`);
  await page.getByLabel('Project name').fill('Copy');
  await page.getByLabel('Code').fill('br-1');
  await page.getByRole('button', {name: 'Save'}).click();
  await page.getByText('Another record already uses one of these unique values.').waitFor();
  await page.getByRole('button', {name: 'Cancel'}).click();

  step('edit sends only the changed values');
  await page.getByRole('link', {name: 'Browser project'}).click();
  await page.getByRole('button', {name: 'Edit'}).click();
  await page.getByLabel('Code').fill('BR-2');
  await page.getByRole('button', {name: 'Save'}).click();
  await page.getByText('BR-2', {exact: true}).waitFor();

  step('delete moves the record to the recycle bin; restore brings it back');
  await page.getByRole('button', {name: 'Delete'}).click();
  await page.getByRole('alertdialog').getByRole('button', {name: 'Delete'}).click();
  await page.getByRole('heading', {level: 1, name: 'Projects'}).waitFor();
  await page.getByRole('button', {name: 'Recycle bin'}).click();
  await page.getByRole('heading', {level: 1, name: 'Projects recycle bin'}).waitFor();
  await page.getByRole('row', {name: /Browser project/}).getByRole('button', {name: 'Restore'}).click();
  await page.getByText('Browser project was restored.').first().waitFor();
  await shot(page, '04-recycle-bin');

  step('record access: an administrator makes Projects private');
  await page.goto(`${origin}/settings/objects/custom:project`);
  await page.getByRole('heading', {level: 1, name: 'Project'}).waitFor();
  await page.getByLabel('Default record access').selectOption('private');
  await page.getByRole('button', {name: 'Save record settings'}).click();
  await page.getByText('Record settings for projects saved.', {exact: false}).first().waitFor();

  step('sharing: share, revoke and transfer from the record page (ADR 0016)');
  await page.goto(`${origin}/o/custom:project`);
  await page.getByRole('link', {name: 'Browser project'}).click();
  const sharing = page.getByRole('region', {name: 'Sharing'});
  await sharing.getByText('This object is private', {exact: false}).waitFor();
  await sharing.getByText('Not shared with anyone yet.').waitFor();
  await sharing.getByLabel('Member').selectOption({label: 'Viewer'});
  await sharing.getByLabel('Access').selectOption('edit');
  await sharing.getByRole('button', {name: 'Share'}).click();
  await sharing.getByRole('row', {name: /Viewer/}).getByText('Read and edit').waitFor();
  await sharing.getByLabel('Share with').selectOption('role');
  await sharing.getByLabel('Role').selectOption({label: 'Sales'});
  await sharing.getByRole('button', {name: 'Share'}).click();
  await sharing.getByRole('row', {name: /Sales/}).waitFor();
  await shot(page, '05-sharing');
  await sharing.getByRole('button', {name: 'Stop sharing with Sales'}).click();
  await sharing.getByText('No longer shared with Sales.').first().waitFor();
  await sharing.getByLabel('Change owner').selectOption({label: 'Riley Rep'});
  await sharing.getByRole('button', {name: 'Transfer'}).click();
  await sharing.getByText('Owned by').getByText('Riley Rep').waitFor();
  await sharing.getByText('Not shared with anyone yet.').waitFor();

  step('history: the record page lists tracked changes, newest first (ADR 0017)');
  const history = page.getByRole('region', {name: 'History'});
  await history.getByRole('row', {name: /Owner/}).getByText('Riley Rep').waitFor();
  await history.getByText('Restored from the recycle bin.').waitFor();
  await history.getByText('Moved to the recycle bin.').waitFor();
  await history.getByText('Created.').waitFor();
  if (await history.getByText('BR-2').count()) throw new Error('untracked fields must not appear in history');
  await shot(page, '06-history');

  step('settings: members and their reporting lines');
  await page.goto(`${origin}/settings`);
  await page.getByRole('link', {name: /Members/}).click();
  await page.getByRole('heading', {level: 1, name: 'Members'}).waitFor();
  const line = page.getByLabel('Riley Rep reports to');
  await line.waitFor();
  const selected = await line.evaluate(element => element.selectedOptions[0]?.textContent ?? '');
  if (selected !== 'Morgan Manager') throw new Error(`Riley Rep should report to Morgan Manager, not ${selected}`);

  step('settings: devices signed in with the app, and revoking one (ADR 0029)');
  await page.goto(`${origin}/settings`);
  await page.getByRole('link', {name: /^Devices/}).click();
  await page.getByRole('heading', {level: 1, name: 'Devices'}).waitFor();
  const watch = page.getByRole('row', {name: /Viewer's watch/});
  await watch.waitFor();
  if (await page.getByText('Former member').count()) throw new Error('the device list did not name its members');
  await watch.getByRole('button', {name: "Revoke Viewer's watch"}).click();
  await watch.getByRole('button', {name: 'Revoke and erase'}).click();
  await page.getByText("Viewer's watch is revoked.", {exact: false}).first().waitFor();
  await watch.getByText('Revoked', {exact: true}).waitFor();
  await shot(page, '07-devices');

  step('global search: instant results, keyboard choice, full results page (ADR 0020)');
  const box = page.getByRole('combobox', {name: 'Search Limiara'});
  await box.fill('browser proj');
  await page.getByRole('option', {name: /Browser project/}).waitFor();
  await shot(page, '08-search-suggestions');
  await box.press('ArrowDown');
  await box.press('Enter');
  await page.getByRole('heading', {level: 1, name: 'Browser project'}).waitFor();
  await page.keyboard.press('Control+KeyK');
  await box.fill('browser');
  await page.getByRole('option', {name: /See all results/}).waitFor();
  await box.press('Enter');
  await page.getByRole('heading', {level: 1, name: 'Results for “browser”'}).waitFor();
  await page.getByRole('region', {name: 'Projects'}).getByRole('link', {name: 'Browser project'}).waitFor();
  await shot(page, '09-search-results');

  step('settings: the query console runs the query language (ADR 0019)');
  await page.goto(`${origin}/settings`);
  await page.getByRole('link', {name: /^Query/}).click();
  await page.getByRole('heading', {level: 1, name: 'Query'}).waitFor();
  await page.getByLabel('Query', {exact: true}).fill("SELECT name, custom:code FROM custom:project WHERE custom:code = 'br-2'");
  await page.getByRole('button', {name: 'Run query'}).click();
  await page.getByRole('cell', {name: 'BR-2'}).waitFor();
  await page.getByLabel('Query', {exact: true}).fill('SELECT name FROM custom:project WHERE');
  await page.getByRole('button', {name: 'Run query'}).click();
  await page.getByRole('alert').getByText(/position/).waitFor();
  await shot(page, '10-query');

  step('validation rules: an administrator adds a rule and the record form shows it (ADR 0021)');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:project')}`);
  const rules = page.getByRole('region', {name: 'Validation rules'});
  await rules.getByRole('button', {name: 'New rule'}).click();
  await rules.getByLabel('Rule name').fill('no_x_codes');
  await rules.getByLabel('Error condition').fill('BEGINS(UPPER(custom:code), "X-"');
  await rules.getByLabel('Error message').fill('Codes cannot start with X-');
  await rules.getByLabel('Show the message on').selectOption({label: 'Code'});
  await rules.getByRole('button', {name: 'Add rule'}).click();
  await rules.getByText(/expected/).first().waitFor();
  await rules.getByLabel('Error condition').fill('BEGINS(UPPER(custom:code), "X-")');
  await rules.getByRole('button', {name: 'Add rule'}).click();
  await rules.getByRole('row', {name: /no_x_codes/}).waitFor();
  await shot(page, '11-validation-rules');
  await page.goto(`${origin}/o/custom:project/new`);
  await page.getByLabel('Project name').fill('Rule check');
  await page.getByLabel('Code').fill('x-7');
  await page.getByRole('button', {name: 'Save'}).click();
  await page.getByText('Codes cannot start with X-').first().waitFor();
  await shot(page, '12-rule-blocks-save');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:project')}`);
  await page.getByRole('button', {name: 'Delete no_x_codes'}).click();
  await page.getByText('No validation rules yet.').waitFor();

  step('formula fields: an administrator adds one and the record page shows its value (ADR 0021)');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:project')}`);
  await page.getByRole('button', {name: 'New field'}).click();
  await page.getByLabel('Field type').selectOption('formula');
  await page.getByLabel('Label', {exact: true}).fill('Code label');
  await page.getByLabel('Formula', {exact: true}).fill('"Code " & custom:code');
  await page.getByRole('button', {name: 'Add field'}).click();
  await page.getByRole('cell', {name: 'custom:code_label'}).waitFor();
  await page.goto(`${origin}/o/custom:project`);
  await page.getByRole('link', {name: 'Browser project'}).click();
  await page.getByRole('heading', {level: 1, name: 'Browser project'}).waitFor();
  await page.getByText('Code BR-2', {exact: true}).first().waitFor();
  await shot(page, '13-formula-field');

  step('automations: an administrator adds one and the next save runs it (ADR 0022)');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:project')}`);
  const automations = page.getByRole('region', {name: 'Automations'});
  await automations.getByRole('button', {name: 'New automation'}).click();
  await automations.getByLabel('Automation name').fill('default_code');
  await automations.getByLabel('Runs when').selectOption('create');
  await automations.getByLabel('Only when').fill('ISBLANK(custom:code)');
  await automations.getByLabel('Field 1', {exact: true}).selectOption({label: 'Code'});
  await automations.getByLabel('Formula for field 1').fill('"AUTO-" &');
  await automations.getByRole('button', {name: 'Add automation'}).click();
  await automations.getByText(/at position/).first().waitFor();
  await automations.getByLabel('Formula for field 1').fill('"AUTO-" & RIGHT(id, 6)');
  await automations.getByRole('button', {name: 'Add automation'}).click();
  await automations.getByRole('row', {name: /default_code/}).waitFor();
  await automations.getByText('Sets Code').waitFor();
  await shot(page, '14-automations');
  await page.goto(`${origin}/o/custom:project/new`);
  await page.getByLabel('Project name').fill('Automated in the browser');
  await page.getByRole('button', {name: 'Save'}).click();
  await page.getByRole('heading', {level: 1, name: 'Automated in the browser'}).waitFor();
  await page.getByText(/^AUTO-/).first().waitFor();
  await shot(page, '15-automation-ran');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:project')}`);
  await page.getByRole('button', {name: 'Delete default_code'}).click();
  await page.getByText('No automations yet.').waitFor();

  step('the Object Manager creates an object and a field');
  await page.goto(`${origin}/settings`);
  await page.getByRole('link', {name: /Object Manager/}).click();
  await page.getByRole('heading', {level: 1, name: 'Object Manager'}).waitFor();
  await page.getByRole('cell', {name: 'custom:project'}).waitFor();
  await page.getByRole('button', {name: 'New object'}).click();
  await page.getByLabel('Label', {exact: true}).fill('Milestone');
  await page.getByLabel('Plural label').fill('Milestones');
  await page.getByRole('button', {name: 'Create object'}).click();
  await page.getByRole('heading', {level: 1, name: 'Milestone'}).waitFor();
  await page.getByText('custom:milestone', {exact: true}).first().waitFor();
  await page.getByRole('button', {name: 'New field'}).click();
  await page.getByLabel('Field type').selectOption('date');
  await page.getByLabel('Label', {exact: true}).fill('Due on');
  await page.getByRole('button', {name: 'Add field'}).click();
  await page.getByRole('cell', {name: 'custom:due_on'}).waitFor();
  await page.getByRole('checkbox', {name: /^Read for /}).first().waitFor();
  await page.getByLabel('Default record access').waitFor();
  await shot(page, '07-object-manager');

  step('the new object joins navigation on revalidation');
  await page.reload();
  await nav.getByRole('link', {name: 'Milestones'}).waitFor();
} catch (error) {
  failed = true;
  console.error(`browser journey failed: ${error instanceof Error ? error.message : String(error)}`);
  if (page) await shot(page, 'failure').catch(() => undefined);
} finally {
  await browser.close();
  server.close();
}
process.exit(failed ? 1 : 0);
