import {createContext, useContext} from 'react';
import {BrowserDraftDatabase} from './browserStore';
import {decodeKey, deriveWorkspaceKey, importBrowserKey} from './crypto';
import type {DraftsContribution, DraftStore} from './types';

/**
 * What the record forms of one workspace draft into. `null` means the host
 * offers no drafts (no `drafts` block), so forms behave as they always did;
 * `unavailable` means drafting is off for this page because the key could
 * not be had, and the form says so. There is no plain fallback.
 */
export type DraftRuntime =
  | {status: 'ready'; store: DraftStore; contribution: DraftsContribution; member: string; workspace: string}
  | {status: 'unavailable'; contribution: DraftsContribution};

export const DraftsContext = createContext<DraftRuntime | null>(null);

export const useDrafts = () => useContext(DraftsContext);

/** Each page load's own writer id, so a tab knows another tab wrote a draft. */
let tab: string | null = null;
export function tabId(): string {
  tab ??= crypto.randomUUID();
  return tab;
}

let database: BrowserDraftDatabase | null = null;

/** The origin's drafts database, or `null` where the browser has no IndexedDB. */
export function browserDrafts(): BrowserDraftDatabase | null {
  if (database) return database;
  if (typeof indexedDB === 'undefined') return null;
  database = new BrowserDraftDatabase(indexedDB);
  return database;
}

type BrowserKey = {browserId: string; key: CryptoKey};
let browserKey: Promise<BrowserKey> | null = null;

/**
 * Fetches the browser's draft key once per page load (ADR 0036 §4) and
 * imports it as a non-extractable key held in memory only. A failure is
 * kept for the page: drafting stays off until the next load.
 */
export function loadBrowserKey(drafts: BrowserDraftDatabase, fetcher: typeof fetch = fetch): Promise<BrowserKey> {
  browserKey ??= (async () => {
    const browserId = await drafts.browserId();
    const response = await fetcher(`/api/v1/me/draft-key?browser=${encodeURIComponent(browserId)}`, {credentials: 'same-origin', cache: 'no-store', headers: {accept: 'application/json'}});
    if (!response.ok) throw new Error('The draft key is unavailable.');
    const body = await response.json() as {key?: unknown};
    if (typeof body.key !== 'string') throw new Error('The draft key is unavailable.');
    return {browserId, key: await importBrowserKey(decodeKey(body.key))};
  })();
  return browserKey;
}

/** Forgets this page's key, as signing out does before the page goes. */
export function forgetBrowserKey() {
  browserKey = null;
}

/** For tests: a fresh page load. */
export function resetDraftRuntimeForTests() {
  browserKey = null;
  database = null;
  tab = null;
}

/**
 * Builds the draft runtime of one workspace from the host's contribution.
 * Only the browser store exists in this slice; a host store (the device
 * bridge) is a later one, so a `host` contribution keeps no drafts yet.
 */
export async function createDraftRuntime({contribution, member, workspace, drafts = browserDrafts(), fetcher}: {contribution: DraftsContribution | null; member: string; workspace: string; drafts?: BrowserDraftDatabase | null; fetcher?: typeof fetch}): Promise<DraftRuntime | null> {
  if (!contribution || contribution.store !== 'browser') return null;
  if (!drafts) return {status: 'unavailable', contribution};
  try {
    const {key} = await loadBrowserKey(drafts, fetcher);
    const workspaceKey = await deriveWorkspaceKey(key, workspace);
    const store = drafts.store(member, workspace, workspaceKey, {maxDrafts: contribution.maxDrafts, maxBytes: contribution.maxBytes, expiryDays: contribution.expiryDays}, tabId());
    return {status: 'ready', store, contribution, member, workspace};
  } catch {
    return {status: 'unavailable', contribution};
  }
}

/**
 * Before anything renders for a signed-in member (ADR 0036 §11): deletes
 * every other member's drafts in this browser, and this member's drafts
 * for workspaces they left. A failure here is safe: those payloads cannot
 * be decrypted with this member's key anyway.
 */
export async function prepareBrowserDrafts(member: string, workspaces: readonly string[], drafts = browserDrafts()): Promise<void> {
  if (!drafts) return;
  try {
    await drafts.prepareFor(member, workspaces);
  } catch {
    // Unreadable to this member whatever happens; nothing to show.
  }
}

/**
 * Signs out (ADR 0036 §11): deletes every draft of the member in this
 * browser first, then posts to `/auth/logout` with the browser id, which
 * revokes the session and deletes this browser's draft key together. If
 * the browser closes part-way, what is left cannot be decrypted.
 */
export async function signOut(member: string, {drafts = browserDrafts(), submit = submitLogout}: {drafts?: BrowserDraftDatabase | null; submit?: (browserId: string | null) => void} = {}): Promise<void> {
  let browserId: string | null = null;
  if (drafts) {
    try {
      await drafts.clearMember(member);
    } catch {
      // The key goes with the session below, so what is left stays unreadable.
    }
    try {
      browserId = await drafts.browserId();
    } catch {
      browserId = null;
    }
  }
  forgetBrowserKey();
  submit(browserId);
}

/** A real form post, so the browser follows the server's redirect like any sign-out. */
function submitLogout(browserId: string | null) {
  const form = document.createElement('form');
  form.method = 'post';
  form.action = browserId ? `/auth/logout?browser=${encodeURIComponent(browserId)}` : '/auth/logout';
  form.hidden = true;
  document.body.append(form);
  form.submit();
}
