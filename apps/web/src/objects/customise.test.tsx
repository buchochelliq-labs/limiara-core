import {describe, expect, it, vi} from 'vitest';
import {createObjectsApiClient, type ObjectAccess} from './api';
import {accessColumns, toggledAccess} from './ObjectManager';

const workspace = '018f0000-0000-7000-8000-000000000001';
const reader: ObjectAccess = {read: true, create: false, edit: false, delete: false, view_all: false, modify_all: false};

describe('customising one object (ADR 0035 G10)', () => {
  it('offers Customise as a column of its own', () => {
    expect(accessColumns.map(column => column.key)).toEqual(['read', 'create', 'edit', 'delete', 'view_all', 'modify_all', 'customise']);
  });

  it('keeps customise independent of the record grants', () => {
    const customiser = toggledAccess(reader, 'customise', true);
    expect(customiser).toEqual({...reader, customise: true});
    // Taking away read takes away the record grants, not customising.
    expect(toggledAccess({...customiser, edit: true}, 'read', false)).toEqual({read: false, create: false, edit: false, delete: false, view_all: false, modify_all: false, customise: true});
    // And a record grant change keeps a customise the server returned.
    expect(toggledAccess(customiser, 'edit', true).customise).toBe(true);
  });

  it('sends customise to the server', async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({access: {...reader, customise: true}}), {status: 200, headers: {'content-type': 'application/json'}})) as unknown as typeof fetch;
    const saved = await createObjectsApiClient(workspace, fetcher).setPermission('custom:project', '018f0000-0000-7000-8000-000000000009', {...reader, customise: true});
    expect(saved.customise).toBe(true);
    const [, init] = (fetcher as unknown as ReturnType<typeof vi.fn>).mock.calls[0] as [string, RequestInit];
    expect(JSON.parse(String(init.body))).toMatchObject({customise: true});
  });
});
