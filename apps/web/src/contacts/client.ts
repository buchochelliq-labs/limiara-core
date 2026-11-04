// Chooses the `ContactsClient` the people workflows use.
//
// Production and the development server talk to the workspace-scoped Contacts
// API. Unit tests (Vite mode `test`) and development-only forced states use
// the fixture client so every state can be rendered deterministically.
import {createContactsApiClient, type ContactsClient} from './api';
import {createFixtureContactsClient, type FixtureWriteState} from './fixture-client';

const writeStates = new Set<FixtureWriteState>(['ready', 'denied', 'error', 'duplicate', 'conflict']);

/** Development-only forced UI states are read from the location; production ignores them. */
export function readForcedState<T extends string>(search: string, key: string, allowed: ReadonlySet<T>): T | undefined {
  if (!import.meta.env.DEV) return undefined;
  const value = new URLSearchParams(search).get(key) as T | null;
  return value && allowed.has(value) ? value : undefined;
}

export function resolveContactsClient(workspaceId: string, search = ''): ContactsClient {
  const forcedWrite = readForcedState(search, 'form-state', writeStates);
  if (import.meta.env.MODE === 'test' || forcedWrite) return createFixtureContactsClient({workspaceId, write: forcedWrite ?? 'ready'});
  return createContactsApiClient(workspaceId);
}
