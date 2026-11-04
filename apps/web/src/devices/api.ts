// Signed-in devices (ADR 0029): a member's own, or everyone's with the
// Manage devices permission. Served by the kernel, never by a feature plugin.

export type DeviceRecord = {
  id: string;
  name: string;
  platform: string;
  member: {id: string; name: string | null};
  lastSeenAt: string;
  createdAt: string;
  revokedAt: string | null;
  revokedBy: {id: string; name: string | null} | null;
  /** `revoked` by a person, or `signed_out` by the device itself. */
  revokedReason: string | null;
  /** The device whose token made the request; a browser is never one. */
  current: boolean;
};

export type DevicesError = 'denied' | 'missing' | 'unavailable';

export class DevicesApiError extends Error {
  constructor(readonly reason: DevicesError) {
    super(reason === 'denied' ? 'You cannot manage these devices.' : reason === 'missing' ? 'This device is not in the workspace.' : 'Devices could not be loaded.');
  }
}

export type DevicesClient = {
  list(signal?: AbortSignal): Promise<DeviceRecord[]>;
  /** Revokes the device; it wipes its copy of the workspace on its next contact. */
  revoke(id: string): Promise<DeviceRecord>;
};

type ApiReference = {id?: unknown; name?: unknown} | null | undefined;
type ApiDevice = {id: string; name: string; platform: string; member?: ApiReference; last_seen_at: string; created_at: string; revoked_at?: string | null; revoked_by?: ApiReference; revoked_reason?: string | null; current?: boolean};

function reference(value: ApiReference): {id: string; name: string | null} | null {
  if (!value || typeof value.id !== 'string') return null;
  return {id: value.id, name: typeof value.name === 'string' ? value.name : null};
}

export function deviceFromApi(device: ApiDevice): DeviceRecord {
  return {
    id: device.id,
    name: device.name,
    platform: device.platform,
    member: reference(device.member) ?? {id: '', name: null},
    lastSeenAt: device.last_seen_at,
    createdAt: device.created_at,
    revokedAt: device.revoked_at ?? null,
    revokedBy: reference(device.revoked_by),
    revokedReason: device.revoked_reason ?? null,
    current: device.current === true,
  };
}

function failure(status: number): DevicesApiError {
  return new DevicesApiError(status === 403 ? 'denied' : status === 404 ? 'missing' : 'unavailable');
}

export function createDevicesApiClient(workspaceId: string, fetcher: typeof fetch = (...args) => fetch(...args)): DevicesClient {
  const base = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/devices`;
  const send = async (url: string, init: RequestInit, signal?: AbortSignal) => {
    let response: Response;
    try {
      response = await fetcher(url, {...init, credentials: 'same-origin', signal});
    } catch (error) {
      if (signal?.aborted) throw error;
      throw new DevicesApiError('unavailable');
    }
    if (!response.ok) throw failure(response.status);
    return response.json() as Promise<unknown>;
  };
  return {
    async list(signal) {
      const body = await send(base, {headers: {accept: 'application/json'}}, signal) as {data?: ApiDevice[]};
      return Array.isArray(body.data) ? body.data.map(deviceFromApi) : [];
    },
    async revoke(id) {
      const body = await send(`${base}/${encodeURIComponent(id)}/revoke`, {method: 'POST', headers: {accept: 'application/json'}});
      return deviceFromApi(body as ApiDevice);
    },
  };
}

const fixtureDevices: DeviceRecord[] = [
  {id: 'dev-phone', name: "Hannah's iPhone", platform: 'ios', member: {id: 'usr-hannah', name: 'Hannah Okafor'}, lastSeenAt: '2026-11-02T08:15:00Z', createdAt: '2026-10-20T10:00:00Z', revokedAt: null, revokedBy: null, revokedReason: null, current: false},
  {id: 'dev-tablet', name: 'Field tablet', platform: 'android', member: {id: 'usr-morgan', name: 'Morgan Lee'}, lastSeenAt: '2026-10-30T16:40:00Z', createdAt: '2026-10-01T09:00:00Z', revokedAt: '2026-10-31T09:00:00Z', revokedBy: {id: 'usr-hannah', name: 'Hannah Okafor'}, revokedReason: 'revoked', current: false},
];

/** Deterministic devices for tests and the showcase. */
export function createFixtureDevicesClient(devices: readonly DeviceRecord[] = fixtureDevices): DevicesClient {
  const held = devices.map(device => ({...device}));
  return {
    async list() { return held.map(device => ({...device})); },
    async revoke(id) {
      const device = held.find(candidate => candidate.id === id);
      if (!device) throw new DevicesApiError('missing');
      if (!device.revokedAt) Object.assign(device, {revokedAt: new Date().toISOString(), revokedBy: {id: 'usr-hannah', name: 'Hannah Okafor'}, revokedReason: 'revoked'});
      return {...device};
    },
  };
}

export function resolveDevicesClient(workspaceId: string): DevicesClient {
  return import.meta.env.MODE === 'test' ? createFixtureDevicesClient() : createDevicesApiClient(workspaceId);
}
