// The browser leg of a device sign-in (ADR 0029), driven against a real
// Keycloak by scripts/test-device-sign-in.sh. The script plays the app: it
// listens on its registered loopback redirect, sends a person through
// `GET /auth/device/login` in Chromium, signs in at Keycloak, catches the
// one-time code on the loopback, redeems it with its PKCE verifier and
// syncs with the tokens. Every sign-in runs in a fresh browser context, so
// no Keycloak session carries over between people.
import {createHash, randomBytes} from 'node:crypto';
import {createServer} from 'node:http';
import {chromium} from '@playwright/test';

const apiBase = required('API_BASE');
const workspace = required('WORKSPACE_ID');
const loopbackPort = Number(required('LOOPBACK_PORT'));
const redirectUri = `http://127.0.0.1:${loopbackPort}/callback`;
const password = 'local-password';

function required(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set`);
  return value;
}

function log(message) {
  console.log(`== ${message}`);
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}

const base64url = (bytes) => Buffer.from(bytes).toString('base64url');
const newVerifier = () => base64url(randomBytes(32));
const challengeOf = (verifier) => base64url(createHash('sha256').update(verifier).digest());

// The app's loopback listener (RFC 8252 §7.3). Each sign-in waits for the
// one redirect that carries its state.
const waiting = new Map();
const loopback = createServer((request, response) => {
  const url = new URL(request.url ?? '/', redirectUri);
  const state = url.searchParams.get('state') ?? '';
  response.writeHead(200, {'Content-Type': 'text/plain; charset=utf-8'});
  response.end('Signed in. You can close this window.');
  const resolve = waiting.get(state);
  if (url.pathname === '/callback' && resolve) {
    waiting.delete(state);
    resolve(url.searchParams);
  }
});
await new Promise((resolve) => loopback.listen(loopbackPort, '127.0.0.1', resolve));

const browser = await chromium.launch();

/** Signs `username` in through the browser; returns the code and the verifier. */
async function signIn(username) {
  const verifier = newVerifier();
  const state = base64url(randomBytes(16));
  const returned = new Promise((resolve, reject) => {
    waiting.set(state, resolve);
    setTimeout(() => reject(new Error(`${username}: no redirect reached the loopback`)), 30_000);
  });
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    const login = new URL('/auth/device/login', apiBase);
    login.search = new URLSearchParams({
      redirect_uri: redirectUri,
      code_challenge: challengeOf(verifier),
      code_challenge_method: 'S256',
      state,
    }).toString();
    await page.goto(login.toString());
    check(new URL(page.url()).hostname === 'auth.localhost', `${username}: the API did not send the browser to Keycloak: ${page.url()}`);
    await page.locator('input[name="username"]').fill(username);
    await page.locator('input[name="password"]').fill(password);
    await page.locator('input[name="password"]').press('Enter');
    const parameters = await returned;
    check(parameters.get('state') === state, `${username}: the state did not come back unchanged`);
    const code = parameters.get('code');
    check(code, `${username}: no code on the redirect`);
    check(!parameters.has('access_token'), `${username}: a token reached the redirect URI`);
    return {code, verifier};
  } finally {
    waiting.delete(state);
    await context.close();
  }
}

async function token(fields) {
  const response = await fetch(new URL('/oauth/token', apiBase), {
    method: 'POST',
    headers: {'Content-Type': 'application/x-www-form-urlencoded'},
    body: new URLSearchParams(fields),
  });
  check(response.headers.get('cache-control') === 'no-store', 'token responses are not cached');
  return {status: response.status, body: await response.json()};
}

const redeem = (code, verifier, name = 'Morgan\'s laptop') =>
  token({
    grant_type: 'authorization_code',
    code,
    code_verifier: verifier,
    redirect_uri: redirectUri,
    workspace_id: workspace,
    device_name: name,
    platform: 'desktop',
  });

async function call(accessToken, method, path, body) {
  const response = await fetch(new URL(`/api/v1/workspaces/${workspace}${path}`, apiBase), {
    method,
    headers: {
      Authorization: `Bearer ${accessToken}`,
      ...(body === undefined ? {} : {'Content-Type': 'application/json'}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  return {status: response.status, body: text ? JSON.parse(text) : null};
}

function expectGrantError(result, description) {
  check(result.status === 400 && result.body.error === 'invalid_grant', `${description}: ${result.status} ${JSON.stringify(result.body)}`);
}

try {
  log('a member signs in at Keycloak and the app redeems the code');
  const first = await signIn('member');
  const issued = await redeem(first.code, first.verifier);
  check(issued.status === 200, `redeem: ${issued.status} ${JSON.stringify(issued.body)}`);
  check(issued.body.token_type === 'Bearer' && issued.body.workspace_id === workspace, `tokens: ${JSON.stringify(issued.body)}`);
  check(Number.isInteger(issued.body.offline_limit_seconds) && issued.body.offline_limit_seconds > 0, 'the offline limit is sent');

  log('the device syncs with its access token');
  const access = issued.body.access_token;
  const objects = await call(access, 'GET', '/objects');
  check(objects.status === 200 && Array.isArray(objects.body.data), `definitions: ${objects.status}`);
  const changes = await call(access, 'GET', '/objects/changes?limit=100');
  check(changes.status === 200 && Array.isArray(changes.body.data), `changes: ${changes.status} ${JSON.stringify(changes.body)}`);
  const revocations = await call(access, 'POST', '/sync/revocations', {records: {}});
  check(revocations.status === 200, `revocations: ${revocations.status}`);
  const users = await call(access, 'GET', '/users');
  check(users.status === 200 && users.body.data.length >= 1, `member directory: ${users.status}`);
  const devices = await call(access, 'GET', '/devices');
  check(devices.status === 403, 'a device does not manage devices');

  log('the refresh token rotates');
  const refreshed = await token({grant_type: 'refresh_token', refresh_token: issued.body.refresh_token});
  check(refreshed.status === 200 && refreshed.body.refresh_token !== issued.body.refresh_token, `refresh: ${JSON.stringify(refreshed.body)}`);
  check((await call(refreshed.body.access_token, 'GET', '/objects')).status === 200, 'the rotated access token works');

  log('a code works once');
  expectGrantError(await redeem(first.code, first.verifier), 'a reused code');

  log('a code needs the verifier of the app that asked for it');
  const second = await signIn('member');
  expectGrantError(await redeem(second.code, newVerifier()), 'a different verifier');
  expectGrantError(await redeem(second.code, second.verifier), 'a code is spent by a wrong attempt');

  log('a person outside the workspace gets no device');
  const outsider = await signIn('nonmember');
  expectGrantError(await redeem(outsider.code, outsider.verifier, 'Outsider\'s phone'), 'a nonmember');

  log('an unregistered redirect URI never reaches Keycloak');
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    const login = new URL('/auth/device/login', apiBase);
    login.search = new URLSearchParams({
      redirect_uri: `http://127.0.0.1:${loopbackPort}/elsewhere`,
      code_challenge: challengeOf(newVerifier()),
      code_challenge_method: 'S256',
      state: 'unregistered',
    }).toString();
    const response = await page.goto(login.toString());
    check(response?.status() === 400, `unregistered redirect: ${response?.status()}`);
    check(new URL(page.url()).host === new URL(apiBase).host, `the browser left the API: ${page.url()}`);
    const problem = JSON.parse(await page.locator('body').innerText());
    check(problem.code === 'auth.device_redirect_invalid', `problem: ${JSON.stringify(problem)}`);
  } finally {
    await context.close();
  }

  const listed = await call(refreshed.body.access_token, 'GET', '/users');
  check(listed.status === 200, 'the device is still signed in after the refused attempts');
  log('device sign-in through Keycloak passed');
} finally {
  await browser.close();
  loopback.close();
}
