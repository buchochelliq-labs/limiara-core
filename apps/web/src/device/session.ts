// The app's sign-in on a device (ADR 0029), answered by the native shell at
// `/api/device/session` rather than by the server. The browser never
// reaches it: the web signs in with its own session.

export type DevicePhase = 'signed_out' | 'browser' | 'syncing' | 'ready' | 'demo';

export type DeviceStatus = {
  phase: DevicePhase;
  /** Why the device is signed out, or what just happened. */
  notice: string | null;
  server: string | null;
  workspace: string | null;
  member: string | null;
  lastSyncedAt: string | null;
  /** Why the last sync did not finish; the app keeps retrying. */
  syncError: string | null;
  lease: {expiresAt: string | null; locked: boolean} | null;
};

export class DeviceSessionError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
  }
}

export type DeviceSessionClient = {
  status(signal?: AbortSignal): Promise<DeviceStatus>;
  /** Opens the system browser to sign in to the workspace at `address`. */
  signIn(address: string): Promise<DeviceStatus>;
  openDemo(): Promise<DeviceStatus>;
  /** Cancels a sign-in, or signs out and deletes the device's copy. */
  signOut(): Promise<DeviceStatus>;
};

export const sessionPath = '/api/device/session';

/** Whether the page runs inside the native device shell's own origin. */
export function inDeviceShell(location?: Pick<Location, 'protocol' | 'hostname'>): boolean {
  const current = location ?? (typeof window === 'undefined' ? undefined : window.location);
  return current?.protocol === 'limiara:' || current?.hostname === 'limiara.localhost';
}

/** The address of a workspace on this server, for the Devices page. */
export function currentWorkspaceAddress(workspaceId: string): string | undefined {
  if (typeof window === 'undefined' || !workspaceId || inDeviceShell()) return undefined;
  return workspaceAddress(window.location.origin, workspaceId);
}

/** The address members enter in the app: the server and the workspace. */
export function workspaceAddress(origin: string, workspaceId: string): string {
  return `${origin.replace(/\/+$/, '')}/workspaces/${workspaceId}`;
}

type ApiStatus = {
  phase?: unknown; notice?: unknown; server?: unknown; workspace?: unknown; member?: unknown;
  last_synced_at?: unknown; sync_error?: unknown;
  lease?: {expires_at?: unknown; locked?: unknown} | null;
};

const phases: DevicePhase[] = ['signed_out', 'browser', 'syncing', 'ready', 'demo'];
const text = (value: unknown): string | null => typeof value === 'string' ? value : null;

export function statusFromApi(body: ApiStatus): DeviceStatus {
  const phase = phases.find(item => item === body.phase) ?? 'signed_out';
  return {
    phase,
    notice: text(body.notice),
    server: text(body.server),
    workspace: text(body.workspace),
    member: text(body.member),
    lastSyncedAt: text(body.last_synced_at),
    syncError: text(body.sync_error),
    lease: body.lease ? {expiresAt: text(body.lease.expires_at), locked: body.lease.locked === true} : null,
  };
}

export function createDeviceSessionClient(fetcher: typeof fetch = fetch): DeviceSessionClient {
  const send = async (init: RequestInit = {}, signal?: AbortSignal) => {
    const response = await fetcher(sessionPath, {...init, signal, headers: {Accept: 'application/json', ...(init.body ? {'Content-Type': 'application/json'} : {})}});
    const body = await response.json().catch(() => ({})) as ApiStatus & {code?: unknown; detail?: unknown};
    if (!response.ok) throw new DeviceSessionError(text(body.code) ?? 'device.unavailable', text(body.detail) ?? 'The app could not do that. Try again.');
    return statusFromApi(body);
  };
  return {
    status: signal => send({}, signal),
    signIn: address => send({method: 'POST', body: JSON.stringify({address})}),
    openDemo: () => send({method: 'POST', body: JSON.stringify({demo: true})}),
    signOut: () => send({method: 'DELETE'}),
  };
}
