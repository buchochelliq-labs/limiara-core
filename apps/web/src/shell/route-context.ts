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

type Delivery = {workspaceId: string; message: RouteContextMessage};

/**
 * Sends a route message when the page kind or ids change, and nothing when
 * they do not, such as when only the search terms change. A message the
 * host refuses or cannot take changes nothing in the shell.
 *
 * Messages go one at a time, in navigation order: the next is sent once the
 * host has answered the last, and only the latest one waiting is sent, so a
 * message the host validates slowly cannot land after a later one. The host
 * also keeps the latest message it received over a slower earlier one (ADR
 * 0034 §9), which covers a page that reloads with a message in flight.
 */
export class RouteReporter {
  private last = '';
  private reported = '';
  private waiting: Delivery | undefined;
  private sending = false;

  constructor(private readonly send: typeof sendRouteContext = sendRouteContext) {}

  report(workspaceId: string, path: string): void {
    this.deliver(workspaceId, routeContextFor(path));
  }

  /**
   * The shell stopped showing a workspace page, as when its features are
   * all disabled or its navigation cannot be loaded: a host that was told a
   * page is told `other`, so it stops treating that page as current. A host
   * that was never told anything, such as the server, is sent nothing.
   */
  leave(): void {
    if (!this.reported) return;
    this.deliver(this.reported, {route: 'other'});
  }

  private deliver(workspaceId: string, message: RouteContextMessage): void {
    const key = JSON.stringify([workspaceId, message]);
    if (key === this.last) return;
    this.last = key;
    this.reported = workspaceId;
    this.waiting = {workspaceId, message};
    this.next();
  }

  private next(): void {
    if (this.sending || !this.waiting) return;
    const {workspaceId, message} = this.waiting;
    this.waiting = undefined;
    this.sending = true;
    let sent: Promise<void>;
    try {
      sent = this.send(workspaceId, message);
    } catch {
      sent = Promise.resolve();
    }
    void sent.catch(() => undefined).finally(() => {
      this.sending = false;
      this.next();
    });
  }
}
