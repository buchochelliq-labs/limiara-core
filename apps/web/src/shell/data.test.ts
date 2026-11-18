import {afterEach, describe, expect, it, vi} from 'vitest';
import {loadWorkspaceNavigation, WorkspaceRequestSequence} from './data';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('workspace activation navigation', () => {
  it('rejects a stale workspace response after a newer request begins', () => {
    const requests = new WorkspaceRequestSequence();
    const previousWorkspace = requests.begin();
    const activeWorkspace = requests.begin();
    expect(requests.isCurrent(previousWorkspace)).toBe(false);
    expect(requests.isCurrent(activeWorkspace)).toBe(true);
  });

  it('returns the opaque activation revision and server-filtered order', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        revision: 'opaque-revision',
        permissions: ['contacts.people.write', 'contacts.people.read'],
        navigation: [
          {id: 'later', label: 'Later', route: '/later', order: 200},
          {id: 'first', label: 'First', route: '/first', order: 10},
        ],
      }),
    }));

    await expect(loadWorkspaceNavigation('workspace-a')).resolves.toEqual({
      revision: 'opaque-revision',
      permissions: ['contacts.people.read', 'contacts.people.write'],
      navigation: [
        {id: 'first', label: 'First', route: '/first', order: 10},
        {id: 'later', label: 'Later', route: '/later', order: 200},
      ],
      drafts: null,
    });
  });

  it('reads the drafts block and refuses a malformed one (ADR 0036 §3)', async () => {
    const respond = (drafts: unknown) => vi.fn().mockResolvedValue({ok: true, status: 200, json: async () => ({revision: 'r', permissions: [], navigation: [], drafts})});
    vi.stubGlobal('fetch', respond({store: 'browser', expiry_days: 7, max_drafts: 50, max_bytes: 1048576}));
    await expect(loadWorkspaceNavigation('workspace-a')).resolves.toMatchObject({drafts: {store: 'browser', expiryDays: 7, maxDrafts: 50, maxBytes: 1048576}});
    for (const bad of [null, {store: 'disk', expiry_days: 7, max_drafts: 50, max_bytes: 1}, {store: 'browser', expiry_days: 0, max_drafts: 50, max_bytes: 1}, {store: 'browser', expiry_days: 91, max_drafts: 50, max_bytes: 1}]) {
      vi.stubGlobal('fetch', respond(bad));
      await expect(loadWorkspaceNavigation('workspace-a')).resolves.toMatchObject({drafts: null});
    }
  });

  it('fails closed instead of restoring fixture navigation on an API error', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: false,
      status: 503,
      json: async () => ({status: 503, detail: 'temporarily unavailable'}),
    }));

    await expect(loadWorkspaceNavigation('workspace-a')).rejects.toMatchObject({status: 503});
  });
});
