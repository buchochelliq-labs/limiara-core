// The current-route context (ADR 0034 §9). When the host's UI contributions
// say it takes route messages, the shell tells it which kind of page it
// shows, by the route pattern's name and ids only: never the address, so
// search terms and filters stay in the page, and never record content. The
// server's contributions never ask, so nothing is sent there.

export type RoutePatternName = 'home' | 'object' | 'new_record' | 'record' | 'search' | 'settings' | 'other';

export type RouteContextMessage = {route: RoutePatternName; object?: string; record_id?: string};

function decoded(segment: string): string | undefined {
  try {
    return decodeURIComponent(segment);
  } catch {
    return undefined;
  }
}

/** The route message for a path; any query string is ignored. */
export function routeContextFor(path: string): RouteContextMessage {
  const pathname = path.split(/[?#]/, 1)[0] || '/';
  if (pathname === '/') return {route: 'home'};
  if (pathname === '/search') return {route: 'search'};
  if (pathname === '/settings' || pathname.startsWith('/settings/')) return {route: 'settings'};
  const match = pathname.match(/^\/o\/([^/]+)(?:\/([^/]+))?$/);
  if (!match) return {route: 'other'};
  const object = decoded(match[1] ?? '');
  if (!object) return {route: 'other'};
  if (match[2] === undefined) return {route: 'object', object};
  if (match[2] === 'new') return {route: 'new_record', object};
  const recordId = decoded(match[2]);
  return recordId ? {route: 'record', object, record_id: recordId} : {route: 'other'};
}

/** Sends one route message to the host, same-origin. */
export async function sendRouteContext(workspaceId: string, message: RouteContextMessage, signal?: AbortSignal): Promise<void> {
  await fetch(`/api/v1/workspaces/${encodeURIComponent(workspaceId)}/ui/route`, {
    method: 'POST',
    headers: {'content-type': 'application/json'},
    body: JSON.stringify(message),
    signal,
  });
}

/**
 * Sends a route message when the page kind or ids change, and nothing when
 * they do not, such as when only the search terms change. A message the
 * host refuses or cannot take changes nothing in the shell.
 */
export class RouteReporter {
  private last = '';

  constructor(private readonly send: typeof sendRouteContext = sendRouteContext) {}

  report(workspaceId: string, path: string): void {
    const message = routeContextFor(path);
    const key = JSON.stringify([workspaceId, message]);
    if (key === this.last) return;
    this.last = key;
    void this.send(workspaceId, message).catch(() => undefined);
  }

  /** Forgets the last message, as when the host stops asking. */
  reset(): void {
    this.last = '';
  }
}
