import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import {createDevicesApiClient, createFixtureDevicesClient, DevicesApiError, type DeviceRecord} from './api';
import {DevicesView, type DevicesLoad} from './DevicesPage';

const workspace = '018f0000-0000-7000-8000-000000000001';

function respond(status: number, body: unknown = {}) {
  return vi.fn(async () => new Response(JSON.stringify(body), {status, headers: {'content-type': 'application/json'}})) as unknown as typeof fetch;
}

const apiDevice = {
  id: 'd1', name: "Ada's phone", platform: 'ios', member: {id: 'u1', name: 'Ada Lovelace'},
  push: 'apns', created_at: '2026-10-01T09:00:00Z', last_seen_at: '2026-11-02T08:00:00Z',
  revoked_at: null, revoked_by: null, revoked_reason: null, current: false,
};

const signedIn: DeviceRecord = {id: 'd1', name: "Ada's <b>phone</b>", platform: 'ios', member: {id: 'u1', name: 'Ada <i>Lovelace</i>'}, lastSeenAt: '2026-11-02T08:00:00Z', createdAt: '2026-10-01T09:00:00Z', revokedAt: null, revokedBy: null, revokedReason: null, current: false};
const revoked: DeviceRecord = {...signedIn, id: 'd2', name: 'Old tablet', platform: 'android', revokedAt: '2026-10-30T10:00:00Z', revokedBy: {id: 'u2', name: 'Grace Hopper'}, revokedReason: 'revoked'};
const signedOut: DeviceRecord = {...signedIn, id: 'd3', name: 'Laptop', platform: 'linux', member: {id: 'u3', name: null}, revokedAt: '2026-10-29T10:00:00Z', revokedBy: null, revokedReason: 'signed_out'};

function render(load: DevicesLoad, extra: {confirming?: string; revoking?: string; notice?: string} = {}) {
  return renderToStaticMarkup(<DevicesView load={load} {...extra} onNavigate={() => undefined} onRetry={() => undefined} onAsk={() => undefined} onCancel={() => undefined} onRevoke={() => undefined} />);
}

describe('signed-in devices', () => {
  it('lists devices from the kernel devices route', async () => {
    const fetcher = respond(200, {data: [apiDevice]});
    const devices = await createDevicesApiClient(workspace, fetcher).list();
    expect(devices).toEqual([{id: 'd1', name: "Ada's phone", platform: 'ios', member: {id: 'u1', name: 'Ada Lovelace'}, lastSeenAt: '2026-11-02T08:00:00Z', createdAt: '2026-10-01T09:00:00Z', revokedAt: null, revokedBy: null, revokedReason: null, current: false}]);
    expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe(`/api/v1/workspaces/${workspace}/devices`);
  });

  it('revokes with a POST to the device', async () => {
    const fetcher = respond(200, {...apiDevice, revoked_at: '2026-11-03T10:00:00Z', revoked_by: {id: 'u2', name: 'Grace Hopper'}, revoked_reason: 'revoked'});
    const device = await createDevicesApiClient(workspace, fetcher).revoke('d/1');
    expect(device.revokedAt).toBe('2026-11-03T10:00:00Z');
    expect(device.revokedBy).toEqual({id: 'u2', name: 'Grace Hopper'});
    const [url, init] = vi.mocked(fetcher).mock.calls[0] ?? [];
    expect(url).toBe(`/api/v1/workspaces/${workspace}/devices/d%2F1/revoke`);
    expect((init as RequestInit).method).toBe('POST');
  });

  it('tells denied, missing and unavailable apart', async () => {
    await expect(createDevicesApiClient(workspace, respond(403)).list()).rejects.toMatchObject({reason: 'denied'});
    await expect(createDevicesApiClient(workspace, respond(404)).revoke('gone')).rejects.toMatchObject({reason: 'missing'});
    await expect(createDevicesApiClient(workspace, respond(503)).list()).rejects.toMatchObject({reason: 'unavailable'});
    const offline = vi.fn(async () => { throw new TypeError('offline'); }) as unknown as typeof fetch;
    await expect(createDevicesApiClient(workspace, offline).list()).rejects.toBeInstanceOf(DevicesApiError);
  });

  it('the fixture revokes once and keeps the first revocation', async () => {
    const client = createFixtureDevicesClient([signedIn]);
    const first = await client.revoke('d1');
    expect(first.revokedReason).toBe('revoked');
    expect((await client.revoke('d1')).revokedAt).toBe(first.revokedAt);
    await expect(client.revoke('nope')).rejects.toMatchObject({reason: 'missing'});
  });

  it('renders each device as text, with its status and a labelled action', () => {
    const html = render({status: 'ready', devices: [signedIn, revoked, signedOut]});
    expect(html).toContain('<h1 tabindex="-1">Devices</h1>');
    expect(html).toContain('Ada&#x27;s &lt;b&gt;phone&lt;/b&gt;');
    expect(html).toContain('Ada &lt;i&gt;Lovelace&lt;/i&gt;');
    expect(html).toContain('iPhone or iPad');
    expect(html).toContain('Signed in');
    expect(html).toContain('Revoked');
    expect(html).toContain('by Grace Hopper');
    expect(html).toContain('Signed out');
    expect(html).toContain('Former member');
    expect(html).toContain('aria-label="Revoke Ada&#x27;s &lt;b&gt;phone&lt;/b&gt;"');
    // Only the signed-in device can be revoked.
    expect(html.match(/>Revoke</g)).toHaveLength(1);
  });

  it('asks before revoking and shows progress', () => {
    const asking = render({status: 'ready', devices: [signedIn]}, {confirming: 'd1'});
    expect(asking).toContain('Revoke and erase');
    expect(asking).toContain('Cancel');
    expect(asking).toContain('role="group"');
    const busy = render({status: 'ready', devices: [signedIn]}, {confirming: 'd1', revoking: 'd1'});
    expect(busy).toContain('aria-busy="true"');
  });

  it('renders loading, empty, denied and unavailable states', () => {
    expect(render({status: 'loading'})).toContain('Loading devices');
    expect(render({status: 'ready', devices: []})).toContain('No devices yet');
    expect(render({status: 'denied'})).toContain('You cannot view these devices');
    const unavailable = render({status: 'unavailable'});
    expect(unavailable).toContain('Devices could not be loaded');
    expect(unavailable).toContain('Try again');
    expect(render({status: 'ready', devices: [signedIn]}, {notice: 'Old tablet is revoked.'})).toContain('aria-live="polite">Old tablet is revoked.');
  });
});
