// Plays the person and the system browser for the headless app probe
// (crates/platform-device/examples/sign_in_probe.rs), driven by
// scripts/test-device-app-sign-in.sh (#385). Each `OPEN <url>` the probe
// prints is a sign-in at Keycloak in a fresh Chromium context; `WAITING
// revoke` is the member revoking the device from the web with their session.
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {chromium} from '@playwright/test';

const required = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set`);
  return value;
};
const probe = required('PROBE');
const apiBase = required('API_BASE');
const workspace = required('WORKSPACE_ID');
const cookie = `platform_session=${required('MEMBER_SESSION')}`;

const browser = await chromium.launch();

async function signIn(url) {
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.goto(url);
    if (new URL(page.url()).hostname !== 'auth.localhost') throw new Error(`not at Keycloak: ${page.url()}`);
    await page.locator('input[name="username"]').fill('member');
    await page.locator('input[name="password"]').fill('local-password');
    await page.locator('input[name="password"]').press('Enter');
    await page.waitForURL((current) => current.hostname === '127.0.0.1', {timeout: 30_000});
  } finally {
    await context.close();
  }
}

async function revokeProbe() {
  const headers = {Cookie: cookie};
  const list = await fetch(`${apiBase}/api/v1/workspaces/${workspace}/devices`, {headers});
  if (list.status !== 200) throw new Error(`device list: ${list.status}`);
  const device = (await list.json()).data.find((item) => item.name === 'Probe laptop' && !item.revoked_at);
  if (!device) throw new Error('the probe device is not listed');
  const revoked = await fetch(`${apiBase}/api/v1/workspaces/${workspace}/devices/${device.id}/revoke`, {method: 'POST', headers});
  if (revoked.status !== 200) throw new Error(`revoke: ${revoked.status}`);
}

const child = spawn(probe, [], {stdio: ['pipe', 'pipe', 'inherit']});
const seen = [];
const lines = createInterface({input: child.stdout});
const failures = [];
lines.on('line', (line) => {
  console.log(line);
  seen.push(line);
  if (line.startsWith('OPEN ')) {
    signIn(line.slice(5)).catch((error) => failures.push(error));
  } else if (line === 'WAITING revoke') {
    revokeProbe()
      .then(() => child.stdin.write('revoked\n'))
      .catch((error) => failures.push(error));
  }
});
const code = await new Promise((resolve) => child.on('exit', resolve));
await browser.close();
if (failures.length) throw failures[0];
if (code !== 0) throw new Error(`the probe exited with ${code}`);
for (const step of ['WIPED', 'SIGNED_OUT']) {
  if (!seen.includes(step)) throw new Error(`the probe never reached ${step}`);
}
console.log('== the app signed in, was wiped when revoked, and signed out');
