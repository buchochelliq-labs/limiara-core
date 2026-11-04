// Workspace user records (ADR 0014): the people `created_by` and
// `updated_by` link to. Served by the kernel, never by a feature plugin.

export type UserRecord = {
  id: string;
  name: string;
  email: string | null;
  roles: string[];
  createdAt: string;
  /** Who the member reports to (ADR 0016). */
  manager: {id: string; name?: string} | null;
};

export type UserLoadError = 'missing' | 'unavailable';

export class UsersApiError extends Error {
  constructor(readonly reason: UserLoadError) {
    super(reason === 'missing' ? 'This user is not a member of the workspace.' : 'User records could not be loaded.');
  }
}

export type UsersClient = {
  getUser(id: string, signal?: AbortSignal): Promise<UserRecord>;
};

type ApiUser = {id: string; name: string; email: string | null; roles: string[]; created_at: string; manager?: {id: string; name?: string} | null};

export function userFromApi(user: ApiUser): UserRecord {
  return {id: user.id, name: user.name, email: user.email ?? null, roles: Array.isArray(user.roles) ? user.roles : [], createdAt: user.created_at, manager: user.manager && typeof user.manager.id === 'string' ? {id: user.manager.id, ...(typeof user.manager.name === 'string' ? {name: user.manager.name} : {})} : null};
}

export function createUsersApiClient(workspaceId: string, fetcher: typeof fetch = (...args) => fetch(...args)): UsersClient {
  const base = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/users`;
  return {
    async getUser(id, signal) {
      let response: Response;
      try {
        response = await fetcher(`${base}/${encodeURIComponent(id)}`, {headers: {accept: 'application/json'}, credentials: 'same-origin', signal});
      } catch (error) {
        if (signal?.aborted) throw error;
        throw new UsersApiError('unavailable');
      }
      if (response.status === 404) throw new UsersApiError('missing');
      if (!response.ok) throw new UsersApiError('unavailable');
      return userFromApi(await response.json() as ApiUser);
    },
  };
}

const fixtureUsers: UserRecord[] = [
  {id: 'usr-hannah', name: 'Hannah Okafor', email: 'hannah.okafor@limiara.example', roles: ['Administrator'], createdAt: '2025-11-04T09:00:00Z', manager: null},
  {id: 'usr-morgan', name: 'Morgan Lee', email: 'morgan.lee@limiara.example', roles: ['Contacts manager'], createdAt: '2026-01-12T14:30:00Z', manager: {id: 'usr-hannah', name: 'Hannah Okafor'}},
];

/** Deterministic users matching the Contacts fixture records' `created_by` and `updated_by`. */
export function createFixtureUsersClient(users: readonly UserRecord[] = fixtureUsers): UsersClient {
  return {
    async getUser(id) {
      const user = users.find(candidate => candidate.id === id);
      if (!user) throw new UsersApiError('missing');
      return user;
    },
  };
}

export function resolveUsersClient(workspaceId: string): UsersClient {
  return import.meta.env.MODE === 'test' ? createFixtureUsersClient() : createUsersApiClient(workspaceId);
}
