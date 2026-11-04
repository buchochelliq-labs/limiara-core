import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import {DeviceSessionView, type DeviceViewProps} from './DeviceGate';
import {createDeviceSessionClient, DeviceSessionError, inDeviceShell, statusFromApi, workspaceAddress, type DeviceStatus} from './session';

const signedOut: DeviceStatus = {phase: 'signed_out', notice: null, server: null, workspace: null, member: null, lastSyncedAt: null, syncError: null, lease: null};
const ready: DeviceStatus = {...signedOut, phase: 'ready', server: 'https://limiara.example.com', workspace: 'Engines', member: 'Ada', lastSyncedAt: '2026-11-03T09:00:00Z', lease: {expiresAt: '2026-11-17T09:00:00Z', locked: false}};
const now = Date.parse('2026-11-03T10:00:00Z');

function render(status: DeviceStatus, extra: Partial<DeviceViewProps> = {}) {
  const noop = () => undefined;
  return renderToStaticMarkup(<DeviceSessionView status={status} address="" now={now} onAddressChange={noop} onSignIn={noop} onDemo={noop} onCancel={noop} onAskSignOut={noop} onKeep={noop} onSignOut={noop} {...extra}><p>workspace content</p></DeviceSessionView>);
}

function respond(status: number, body: unknown) {
  return vi.fn(async () => new Response(JSON.stringify(body), {status, headers: {'content-type': 'application/json'}})) as unknown as typeof fetch;
}

describe('device sign-in', () => {
  it('knows the device shell by its origin', () => {
    expect(inDeviceShell({protocol: 'limiara:', hostname: 'localhost'})).toBe(true);
    expect(inDeviceShell({protocol: 'http:', hostname: 'limiara.localhost'})).toBe(true);
    expect(inDeviceShell({protocol: 'https:', hostname: 'limiara.example.com'})).toBe(false);
    expect(workspaceAddress('https://limiara.example.com/', 'w1')).toBe('https://limiara.example.com/workspaces/w1');
  });

  it('reads the shell’s session route', async () => {
    const fetcher = respond(200, {phase: 'ready', workspace: 'Engines', member: 'Ada', last_synced_at: '2026-11-03T09:00:00Z', lease: {expires_at: '2026-11-17T09:00:00Z', locked: false}});
    const status = await createDeviceSessionClient(fetcher).signIn('https://limiara.example.com/workspaces/w1');
    expect(status.phase).toBe('ready');
    expect(status.lease).toEqual({expiresAt: '2026-11-17T09:00:00Z', locked: false});
    expect(fetcher).toHaveBeenCalledWith('/api/device/session', expect.objectContaining({method: 'POST', body: JSON.stringify({address: 'https://limiara.example.com/workspaces/w1'})}));
    expect(statusFromApi({phase: 'something else'}).phase).toBe('signed_out');
    await expect(createDeviceSessionClient(respond(400, {code: 'device.address_invalid', detail: 'enter the workspace address'})).signIn('x'))
      .rejects.toEqual(new DeviceSessionError('device.address_invalid', 'enter the workspace address'));
  });

  it('signs in through the browser, with no password field', () => {
    const html = render({...signedOut, notice: 'Signed out. This device’s copy of the workspace was deleted.'}, {error: 'enter the workspace address'});
    expect(html).toContain('Sign in to your workspace');
    expect(html).toContain('<label for="device-address">Workspace address</label>');
    expect(html).toContain('aria-invalid="true"');
    expect(html).toContain('role="alert"');
    expect(html).toContain('Continue in your browser');
    expect(html).toContain('copy of the workspace was deleted');
    expect(html).not.toMatch(/type="password"|password"/);
    expect(html).not.toContain('workspace content');
  });

  it('waits for the browser and the first sync', () => {
    expect(render({...signedOut, phase: 'browser', server: 'https://limiara.example.com'})).toContain('Finish signing in in your browser');
    const syncing = render({...signedOut, phase: 'syncing', workspace: 'Engines', member: 'Ada', syncError: 'timed out'});
    expect(syncing).toContain('Bringing Engines to this device');
    expect(syncing).toContain('aria-label="First sync in progress"');
    expect(syncing).toContain('The server could not be reached');
  });

  it('opens the workspace under its sync state and sign-out', () => {
    const html = render(ready);
    expect(html).toContain('workspace content');
    expect(html).toContain('Sign out of this device');
    expect(html).not.toContain('to keep working offline');
    expect(render(ready, {confirmingSignOut: true})).toContain('Sign out and delete this device');
  });

  it('warns before the offline lease runs out, and when it has', () => {
    expect(render({...ready, lease: {expiresAt: '2026-11-04T09:00:00Z', locked: false}})).toContain('to keep working offline');
    const locked = render({...ready, lease: {expiresAt: '2026-11-02T09:00:00Z', locked: true}});
    expect(locked).toContain('This device has been offline too long');
    expect(locked).not.toContain('to keep working offline');
  });

  it('holds the sign-in while the old workspace is deleted', () => {
    const html = render({...signedOut, phase: 'signing_out', workspace: 'Engines'});
    expect(html).toContain('Signing out');
    expect(html).toContain('aria-label="Deleting this device');
    expect(html).not.toContain('Continue in your browser');
    expect(statusFromApi({phase: 'signing_out'}).phase).toBe('signing_out');
  });

  it('marks the demonstration workspace', () => {
    const html = render({...signedOut, phase: 'demo', workspace: 'Demonstration'});
    expect(html).toContain('demonstration, nothing is saved');
    expect(html).toContain('Leave the demonstration');
  });
});
