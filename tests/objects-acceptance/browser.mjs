// Browser layer of the ADR 0015 object engine acceptance journey.
//
//   node tests/objects-acceptance/browser.mjs
//
// Serves the production web build (apps/web/dist) same-origin with the real
// API (API_BASE), signs in with the seeded administrator session
// (SESSION_TOKEN), and drives the object pages end to end: navigation from the
// UI contribution endpoint, the object home, create, edit, delete and restore
// from the recycle bin, quick actions defined in the Object Manager and run
// from record pages, then the Object Manager creating an object and a
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

  step('the new-record modal opens on its first field and Escape returns to the list');
  const newProject = page.getByRole('button', {name: 'New project'});
  await newProject.click();
  let dialog = page.getByRole('dialog', {name: 'New project'});
  await dialog.waitFor();
  if (!(await dialog.getByLabel('Project name').evaluate(element => element === document.activeElement))) throw new Error('the modal must focus its first field');
  await page.keyboard.press('Escape');
  await dialog.waitFor({state: 'detached'});
  if (!(await newProject.evaluate(element => element === document.activeElement))) throw new Error('closing the modal must return focus to New project');

  step('Save & New keeps the modal open on a blank form for the next record');
  await newProject.click();
  await dialog.getByLabel('Project name').fill('Batch one');
  await dialog.getByRole('button', {name: 'Save & New'}).click();
  await dialog.getByText('Batch one was saved.').waitFor();
  if (await dialog.getByLabel('Project name').inputValue() !== '') throw new Error('Save & New must start a blank form');
  if (!(await dialog.getByLabel('Project name').evaluate(element => element === document.activeElement))) throw new Error('Save & New must focus the blank form');
  await dialog.getByLabel('Project name').fill('Batch two');
  await shot(page, '02-save-and-new');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await page.getByRole('heading', {level: 1, name: 'Batch two'}).waitFor();
  await page.goto(`${origin}/o/custom:project`);
  await page.getByRole('link', {name: 'Batch one'}).waitFor();

  step('create a record from field metadata');
  await page.getByRole('button', {name: 'New project'}).click();
  dialog = page.getByRole('dialog', {name: 'New project'});
  await dialog.getByLabel('Project name').fill('Browser project');
  await dialog.getByLabel('Code').fill('BR-1');
  await dialog.getByLabel('Status').selectOption('won');
  await dialog.getByLabel('Starts on').fill('2026-12-01');
  await dialog.getByLabel('Client', {exact: true}).selectOption({label: 'Analytical Engines'});
  await shot(page, '02-new-record');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await page.getByRole('heading', {level: 1, name: 'Browser project'}).waitFor();
  await page.getByText('Won', {exact: true}).waitFor();
  await page.getByRole('link', {name: 'Analytical Engines'}).waitFor();
  await shot(page, '03-record');

  step('New on a related list fills the parent in and stays on it (ADR 0032)');
  await page.getByRole('link', {name: 'Analytical Engines'}).click();
  await page.getByRole('heading', {level: 1, name: 'Analytical Engines'}).waitFor();
  const related = page.getByRole('region', {name: 'Projects'});
  await related.getByRole('link', {name: 'Browser project'}).waitFor();
  await related.getByRole('button', {name: 'New project'}).click();
  dialog = page.getByRole('dialog', {name: 'New project'});
  if (await dialog.getByLabel('Client', {exact: true}).inputValue() !== 'Analytical Engines') throw new Error('the related form must name its parent');
  if (!(await dialog.getByLabel('Client', {exact: true}).evaluate(element => element.readOnly))) throw new Error('the parent must not be changed from its related list');
  await dialog.getByLabel('Project name').fill('Client project');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await dialog.waitFor({state: 'detached'});
  await page.getByRole('heading', {level: 1, name: 'Analytical Engines'}).waitFor();
  await related.getByRole('link', {name: 'Client project'}).waitFor();
  await shot(page, '03-related-new');

  step('global New lists what the member may create and opens its form anywhere');
  const globalNew = page.getByRole('button', {name: 'New', exact: true});
  await globalNew.click();
  const menu = page.getByRole('group', {name: 'Create a record'});
  await menu.getByRole('button', {name: 'Project', exact: true}).waitFor();
  if (await menu.getByRole('button', {name: 'Client', exact: true}).count()) throw new Error('objects outside navigation must not be offered');
  await page.keyboard.press('Escape');
  await menu.waitFor({state: 'detached'});
  await globalNew.click();
  await menu.getByRole('button', {name: 'Project', exact: true}).click();
  dialog = page.getByRole('dialog', {name: 'New project'});
  await dialog.getByLabel('Project name').fill('Global project');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await page.getByRole('heading', {level: 1, name: 'Global project'}).waitFor();
  await page.goto(`${origin}/o/custom:project`);
  await page.getByRole('link', {name: 'Browser project'}).click();
  await page.getByRole('heading', {level: 1, name: 'Browser project'}).waitFor();

  step('a duplicate unique value is explained, not lost');
  await page.goto(`${origin}/o/custom:project/new`);
  await page.getByLabel('Project name').fill('Copy');
  await page.getByLabel('Code').fill('br-1');
  await page.getByRole('button', {name: 'Save', exact: true}).click();
  await page.getByText('Another record already uses one of these unique values.').waitFor();
  await page.getByRole('button', {name: 'Cancel'}).click();

  step('edit sends only the changed values');
  await page.getByRole('link', {name: 'Browser project'}).click();
  await page.getByRole('button', {name: 'Edit'}).click();
  dialog = page.getByRole('dialog', {name: 'Edit Browser project'});
  await dialog.getByLabel('Code').fill('BR-2');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await dialog.waitFor({state: 'detached'});
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
  await page.getByRole('button', {name: 'Save', exact: true}).click();
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
  await page.getByRole('button', {name: 'Save', exact: true}).click();
  await page.getByRole('heading', {level: 1, name: 'Automated in the browser'}).waitFor();
  await page.getByText(/^AUTO-/).first().waitFor();
  await shot(page, '15-automation-ran');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:project')}`);
  await page.getByRole('button', {name: 'Delete default_code'}).click();
  await page.getByText('No automations yet.').waitFor();

  step('quick actions: an administrator adds an update action and a member runs it from a record (ADR 0032)');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:project')}`);
  let quick = page.getByRole('region', {name: 'Quick actions'});
  await quick.getByText('No quick actions yet.').waitFor();
  await quick.getByRole('button', {name: 'New quick action'}).click();
  await quick.getByLabel('Label', {exact: true}).fill('Mark won');
  if (await quick.getByLabel('Action name').inputValue() !== 'mark_won') throw new Error('the action name must follow its label');
  if (await quick.getByRole('checkbox', {name: 'Code label'}).count()) throw new Error('formula fields cannot be chosen');
  await quick.getByRole('button', {name: 'Add preset'}).click();
  await quick.getByLabel('Preset field 1').selectOption({label: 'Status'});
  await quick.getByLabel('Value for preset 1').selectOption('won');
  await quick.getByRole('button', {name: 'Add quick action'}).click();
  await quick.getByText('choose 1 to 20 fields').first().waitFor();
  await quick.getByRole('checkbox', {name: 'Code', exact: true}).check();
  await quick.getByRole('button', {name: 'Add quick action'}).click();
  await quick.getByRole('row', {name: /Mark won/}).getByText('Status = Won').waitFor();
  await shot(page, '16-quick-action-manager');
  await page.goto(`${origin}/o/custom:project`);
  await page.getByRole('link', {name: 'Batch one'}).click();
  await page.getByRole('heading', {level: 1, name: 'Batch one'}).waitFor();
  await page.getByRole('region', {name: 'Details'}).getByText('Open', {exact: true}).waitFor();
  await page.getByRole('button', {name: 'Mark won'}).click();
  dialog = page.getByRole('dialog', {name: 'Mark won'});
  await dialog.getByLabel('Code').waitFor();
  for (const hidden of ['Project name', 'Status', 'Starts on', 'Client']) {
    if (await dialog.getByLabel(hidden, {exact: true}).count()) throw new Error(`the action form must show only its fields, not ${hidden}`);
  }
  if (await dialog.getByRole('button', {name: 'Save & New'}).count()) throw new Error('an update action offers no Save & New');
  await dialog.getByLabel('Code').fill('BO-1');
  await shot(page, '17-quick-action-update');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await dialog.waitFor({state: 'detached'});
  await page.getByRole('region', {name: 'Details'}).getByText('Won', {exact: true}).waitFor();
  await page.getByRole('region', {name: 'Details'}).getByText('BO-1', {exact: true}).waitFor();

  step('quick actions: a create action on Client makes a project through its Client field');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:client')}`);
  quick = page.getByRole('region', {name: 'Quick actions'});
  await quick.getByRole('button', {name: 'New quick action'}).click();
  await quick.getByLabel('Label', {exact: true}).fill('Start project');
  await quick.getByLabel('Kind').selectOption('create');
  await quick.getByLabel('Creates').selectOption({label: 'Project (Client)'});
  if (await quick.getByRole('checkbox', {name: 'Client', exact: true}).count()) throw new Error('the reference field is set by the action, not chosen');
  await quick.getByRole('checkbox', {name: 'Project name'}).check();
  await quick.getByRole('checkbox', {name: 'Code', exact: true}).check();
  await quick.getByRole('button', {name: 'Add preset'}).click();
  await quick.getByLabel('Preset field 1').selectOption({label: 'Status'});
  await quick.getByLabel('Value for preset 1').selectOption('won');
  await quick.getByRole('button', {name: 'Add quick action'}).click();
  await quick.getByRole('row', {name: /Start project/}).getByText('Creates Project (Client)').waitFor();
  await page.goto(`${origin}/o/custom:client`);
  await page.getByRole('link', {name: 'Analytical Engines'}).click();
  await page.getByRole('heading', {level: 1, name: 'Analytical Engines'}).waitFor();
  await page.getByRole('button', {name: 'Start project'}).click();
  dialog = page.getByRole('dialog', {name: 'Start project'});
  if (await dialog.getByLabel('Client', {exact: true}).inputValue() !== 'Analytical Engines') throw new Error('the create action must name its parent');
  if (!(await dialog.getByLabel('Client', {exact: true}).evaluate(element => element.readOnly))) throw new Error('the parent must not be changed from a quick action');
  if (await dialog.getByLabel('Status', {exact: true}).count()) throw new Error('a preset the action does not show stays hidden');
  await dialog.getByLabel('Project name').fill('Quick project');
  await dialog.getByLabel('Code').fill('QP-1');
  await shot(page, '18-quick-action-create');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await dialog.waitFor({state: 'detached'});
  await page.getByRole('heading', {level: 1, name: 'Analytical Engines'}).waitFor();
  await page.getByRole('region', {name: 'Projects'}).getByRole('link', {name: 'Quick project'}).click();
  await page.getByRole('heading', {level: 1, name: 'Quick project'}).waitFor();
  await page.getByRole('region', {name: 'Details'}).getByText('Won', {exact: true}).waitFor();
  await page.getByRole('link', {name: 'Analytical Engines'}).waitFor();

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

  step('related-to fields: a milestone points at a client or a project, chosen in the form (ADR 0033)');
  await page.goto(`${origin}/settings/objects/${encodeURIComponent('custom:milestone')}`);
  await page.getByRole('button', {name: 'New field'}).click();
  const newField = page.getByRole('form', {name: 'New field'});
  await newField.getByLabel('Field type').selectOption('related_to');
  await newField.getByLabel('Label', {exact: true}).fill('Regarding');
  await newField.getByRole('checkbox', {name: 'Client', exact: true}).check();
  await newField.getByRole('checkbox', {name: 'Project', exact: true}).check();
  await shot(page, '19-related-to-field');
  await newField.getByRole('button', {name: 'Add field'}).click();
  await page.getByRole('cell', {name: 'custom:regarding'}).waitFor();
  await page.getByText('Related to (custom:client, custom:project)').waitFor();
  await page.goto(`${origin}/o/custom:milestone`);
  await page.getByRole('button', {name: 'New milestone'}).click();
  dialog = page.getByRole('dialog', {name: 'New milestone'});
  await dialog.getByLabel('Milestone name').fill('Engine review');
  await dialog.getByLabel('Regarding: object').selectOption({label: 'Client'});
  await dialog.getByLabel('Regarding: Client', {exact: true}).selectOption({label: 'Analytical Engines'});
  await shot(page, '20-related-to-picker');
  await dialog.getByRole('button', {name: 'Save', exact: true}).click();
  await dialog.waitFor({state: 'detached'});
  await page.getByRole('heading', {level: 1, name: 'Engine review'}).waitFor();
  await page.getByRole('region', {name: 'Details'}).getByRole('link', {name: 'Analytical Engines'}).click();
  await page.getByRole('heading', {level: 1, name: 'Analytical Engines'}).waitFor();
  await page.getByRole('region', {name: 'Milestones'}).getByRole('link', {name: 'Engine review'}).waitFor();
} catch (error) {
  failed = true;
  console.error(`browser journey failed: ${error instanceof Error ? error.message : String(error)}`);
  if (page) await shot(page, 'failure').catch(() => undefined);
} finally {
  await browser.close();
  server.close();
}
process.exit(failed ? 1 : 0);
