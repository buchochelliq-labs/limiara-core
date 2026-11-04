import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it, vi} from 'vitest';
import {createFixtureUsersClient, createUsersApiClient, UsersApiError} from './api';
import {UserRecordView} from './UserRecordPage';

const workspace = '018f0000-0000-7000-8000-000000000001';

function respond(status: number, body: unknown = {}) {
  return vi.fn(async () => new Response(JSON.stringify(body), {status, headers: {'content-type': 'application/json'}})) as unknown as typeof fetch;
}

describe('workspace user records', () => {
  it('reads a member from the kernel users route', async () => {
    const fetcher = respond(200, {id: 'u1', name: 'Grace Hopper', email: 'grace@example.org', roles: ['Administrator'], created_at: '2026-01-02T03:04:05Z'});
    const user = await createUsersApiClient(workspace, fetcher).getUser('u/1');
    expect(user).toEqual({id: 'u1', name: 'Grace Hopper', email: 'grace@example.org', roles: ['Administrator'], createdAt: '2026-01-02T03:04:05Z', manager: null});
    expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe(`/api/v1/workspaces/${workspace}/users/u%2F1`);
  });

  it('tells a missing member apart from an outage', async () => {
    await expect(createUsersApiClient(workspace, respond(404)).getUser('gone')).rejects.toMatchObject({reason: 'missing'});
    await expect(createUsersApiClient(workspace, respond(503)).getUser('u1')).rejects.toMatchObject({reason: 'unavailable'});
    await expect(createFixtureUsersClient().getUser('nobody')).rejects.toBeInstanceOf(UsersApiError);
  });

  it('renders every state with an accessible heading and no raw markup from data', () => {
    const ready = renderToStaticMarkup(<UserRecordView load={{status: 'ready', user: {id: 'u1', name: 'Grace <b>Hopper</b>', email: 'grace@example.org', roles: ['Administrator', 'Contacts manager'], createdAt: '2026-01-02T03:04:05Z', manager: {id: 'u2', name: 'Ada <i>Lovelace</i>'}}}} title="Grace <b>Hopper</b>" onRetry={() => undefined} />);
    expect(ready).toContain('<h1 tabindex="-1">Grace &lt;b&gt;Hopper&lt;/b&gt;</h1>');
    expect(ready).toContain('<a href="mailto:grace@example.org">grace@example.org</a>');
    expect(ready).toContain('Administrator, Contacts manager');
    expect(ready).toContain('User since');
    expect(ready).toContain('Reports to');
    expect(ready).toContain('Ada &lt;i&gt;Lovelace&lt;/i&gt;</a>');
    expect(renderToStaticMarkup(<UserRecordView load={{status: 'loading'}} title="User" onRetry={() => undefined} />)).toContain('Loading user');
    expect(renderToStaticMarkup(<UserRecordView load={{status: 'missing'}} title="User" onRetry={() => undefined} />)).toContain('User not available');
    expect(renderToStaticMarkup(<UserRecordView load={{status: 'unavailable'}} title="User" onRetry={() => undefined} />)).toContain('Try again');
  });
});
