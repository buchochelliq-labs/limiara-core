/** The workspace's record draft settings (ADR 0036 §9): any member reads them; `objects.manage` changes them. */
export type DraftSettings = {
  expiryDays: number;
  defaultExpiryDays: number;
  minExpiryDays: number;
  maxExpiryDays: number;
  version: number;
  updatedAt: string | null;
  updatedBy: {id: string; name?: string | null} | null;
};

export type DraftSettingsProblem = 'invalid' | 'conflict' | 'denied' | 'unavailable';

export class DraftSettingsError extends Error {
  constructor(readonly problem: DraftSettingsProblem, readonly detail = '') {
    super(detail || problem);
  }
}

export type DraftSettingsClient = {
  get(signal?: AbortSignal): Promise<DraftSettings>;
  update(expiryDays: number, expectedVersion: number): Promise<DraftSettings>;
};

type ApiSettings = {expiry_days: number; default_expiry_days: number; min_expiry_days: number; max_expiry_days: number; version: number; updated_at: string | null; updated_by: {id: string; name?: string | null} | null};

function fromApi(value: ApiSettings): DraftSettings {
  return {expiryDays: value.expiry_days, defaultExpiryDays: value.default_expiry_days, minExpiryDays: value.min_expiry_days, maxExpiryDays: value.max_expiry_days, version: value.version, updatedAt: value.updated_at, updatedBy: value.updated_by};
}

export function createDraftSettingsClient(workspaceId: string, fetcher: typeof fetch = (...args) => fetch(...args)): DraftSettingsClient {
  const url = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/objects/draft-settings`;
  async function read(response: Response): Promise<DraftSettings> {
    if (response.ok) return fromApi(await response.json() as ApiSettings);
    const body = await response.json().catch(() => ({})) as {code?: string; errors?: {code?: string; detail?: string}[]};
    if (response.status === 400) throw new DraftSettingsError('invalid', body.errors?.[0]?.detail ?? '');
    if (response.status === 409) throw new DraftSettingsError('conflict');
    if (response.status === 403 || response.status === 404) throw new DraftSettingsError('denied');
    throw new DraftSettingsError('unavailable');
  }
  return {
    get: async signal => read(await fetcher(url, {signal, headers: {accept: 'application/json'}})),
    update: async (expiryDays, expectedVersion) => read(await fetcher(url, {method: 'PUT', headers: {'content-type': 'application/json', accept: 'application/json'}, body: JSON.stringify({expiry_days: expiryDays, expected_version: expectedVersion})})),
  };
}
