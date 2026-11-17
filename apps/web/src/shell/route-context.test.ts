import {afterEach, describe, expect, it, vi} from 'vitest';
import {loadWorkspaceNavigation} from './data';
import {RouteReporter, routeContextFor, sendRouteContext} from './route-context';

afterEach(() => {
  vi.unstubAllGlobals();
});

/** Lets pending sends answer and the next one start. */
async function settled(): Promise<void> {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
}

describe('route context', () => {
  it('names the route pattern and ids, never the address', () => {
    expect(routeContextFor('/')).toEqual({route: 'home'});
    expect(routeContextFor('/o/custom:account')).toEqual({route: 'object', object: 'custom:account'});
    expect(routeContextFor('/o/custom:account/new')).toEqual({route: 'new_record', object: 'custom:account'});
    expect(routeContextFor('/o/custom:account/a01000000000001xyz')).toEqual({route: 'record', object: 'custom:account', record_id: 'a01000000000001xyz'});
    expect(routeContextFor('/o/example.tracker%3Aticket')).toEqual({route: 'object', object: 'example.tracker:ticket'});
    // Search terms and filters stay in the page.
    expect(routeContextFor('/search?q=acme%20secret')).toEqual({route: 'search'});
    expect(routeContextFor('/o/custom:account?filter=vip')).toEqual({route: 'object', object: 'custom:account'});
    expect(routeContextFor('/settings/objects/custom:account')).toEqual({route: 'settings'});
    expect(routeContextFor('/contacts/people/ada-lovelace')).toEqual({route: 'other'});
    expect(routeContextFor('/o/%E0%A4%A')).toEqual({route: 'other'});
    expect(routeContextFor('/o/custom:account/a/b')).toEqual({route: 'other'});
  });

  it('sends a same-origin JSON message to the host', async () => {
    const fetch = vi.fn().mockResolvedValue({ok: true, status: 204});
    vi.stubGlobal('fetch', fetch);
    await sendRouteContext('workspace-a', {route: 'record', object: 'custom:account', record_id: 'a01000000000001xyz'});
    expect(fetch).toHaveBeenCalledWith('/api/v1/workspaces/workspace-a/ui/route', expect.objectContaining({
      method: 'POST',
      headers: {'content-type': 'application/json'},
      body: JSON.stringify({route: 'record', object: 'custom:account', record_id: 'a01000000000001xyz'}),
    }));
  });

  it('reports only changes, and survives a host that refuses', async () => {
    const send = vi.fn().mockRejectedValue(new Error('offline'));
    const reporter = new RouteReporter(send);
    reporter.report('workspace-a', '/o/custom:account');
    await settled();
    reporter.report('workspace-a', '/o/custom:account?filter=other');
    reporter.report('workspace-a', '/o/custom:account/new');
    await settled();
    reporter.report('workspace-b', '/o/custom:account/new');
    await settled();
    expect(send.mock.calls).toEqual([
      ['workspace-a', {route: 'object', object: 'custom:account'}],
      ['workspace-a', {route: 'new_record', object: 'custom:account'}],
      ['workspace-b', {route: 'new_record', object: 'custom:account'}],
    ]);
  });

  it('delivers one message at a time, so the latest navigation lands last', async () => {
    // The host answers in whatever order it likes; a record page it is
    // still checking must not land after the home page that followed it.
    const answers: Array<() => void> = [];
    const send = vi.fn(() => new Promise<void>(resolve => { answers.push(resolve); }));
    const reporter = new RouteReporter(send);
    reporter.report('workspace-a', '/o/custom:account/a01000000000001xyz');
    reporter.report('workspace-a', '/o/custom:account');
    reporter.report('workspace-a', '/');
    // Only the first is in flight; the object page was overtaken while waiting.
    expect(send).toHaveBeenCalledTimes(1);
    answers.shift()?.();
    await settled();
    expect(send.mock.calls).toEqual([
      ['workspace-a', {route: 'record', object: 'custom:account', record_id: 'a01000000000001xyz'}],
      ['workspace-a', {route: 'home'}],
    ]);
    answers.shift()?.();
    await settled();
    expect(send).toHaveBeenCalledTimes(2);
  });

  it('tells a host it told a page that the shell left it', async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const reporter = new RouteReporter(send);
    // Never told anything, such as the server: nothing is sent.
    reporter.leave();
    await settled();
    expect(send).not.toHaveBeenCalled();
    reporter.report('workspace-a', '/o/custom:account/a01000000000001xyz');
    await settled();
    // The workspace's last feature was disabled, or navigation failed.
    reporter.leave();
    reporter.leave();
    await settled();
    expect(send.mock.calls).toEqual([
      ['workspace-a', {route: 'record', object: 'custom:account', record_id: 'a01000000000001xyz'}],
      ['workspace-a', {route: 'other'}],
    ]);
    // Back on the same page, the host hears it again.
    reporter.report('workspace-a', '/o/custom:account/a01000000000001xyz');
    await settled();
    expect(send).toHaveBeenCalledTimes(3);
  });

  it('is asked for only by a host whose contributions say so', async () => {
    const contributions = (extra: Record<string, unknown>) => vi.fn().mockResolvedValue({ok: true, json: async () => ({revision: 'r', permissions: [], navigation: [], ...extra})});
    vi.stubGlobal('fetch', contributions({}));
    expect((await loadWorkspaceNavigation('workspace-a')).routeContext).toBeUndefined();
    vi.stubGlobal('fetch', contributions({route_context: 'yes'}));
    expect((await loadWorkspaceNavigation('workspace-a')).routeContext).toBeUndefined();
    vi.stubGlobal('fetch', contributions({route_context: true}));
    expect((await loadWorkspaceNavigation('workspace-a')).routeContext).toBe(true);
  });
});
