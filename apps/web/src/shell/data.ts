import type {NavigationContribution, SessionUser, ShellViewModel, Workspace} from './types';

type ApiProblem = {detail?: string; status?: number};

const fixtureWorkspaces: Workspace[] = [
  {id: '018f0000-0000-7000-8000-000000000001', name: 'Northstar Services'},
  {id: '018f0000-0000-7000-8000-000000000002', name: 'Riverview Property Group'},
];

export const fixtureUser: SessionUser = {id: 'local-user', displayName: 'Hannah Carter', role: 'Administrator'};

export const fixtureNavigation: NavigationContribution[] = [
  {id: 'contacts', label: 'People', route: '/contacts', icon: 'people', order: 100},
  {id: 'contacts-organisations', label: 'Organisations', route: '/contacts/organisations', icon: 'organisations', order: 110},
];

export class WorkspaceRequestSequence {
  private latest = 0;

  begin() {
    this.latest += 1;
    return this.latest;
  }

  isCurrent(sequence: number) {
    return sequence === this.latest;
  }
}

async function api<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {headers: {'content-type': 'application/json'}, signal});
  if (!response.ok) {
    const problem = await response.json().catch(() => ({})) as ApiProblem;
    throw {...problem, status: response.status};
  }
  return response.json() as Promise<T>;
}

export async function loadShellContext(signal?: AbortSignal): Promise<{workspaces: Workspace[]; user: SessionUser}> {
  try {
    const [workspaceResult, userResult] = await Promise.all([
      api<{data: Workspace[]}>('/workspaces', signal),
      api<{user: {id: string; display_name: string}}>('/me', signal),
    ]);
    return {
      workspaces: workspaceResult.data,
      user: {id: userResult.user.id, displayName: userResult.user.display_name, role: 'Administrator'},
    };
  } catch (error) {
    if (signal?.aborted) throw error;
    if (import.meta.env.DEV) return {workspaces: fixtureWorkspaces, user: fixtureUser};
    throw error;
  }
}

export type WorkspaceActivation = {navigation: NavigationContribution[]; permissions: string[]; revision: string};

export async function loadWorkspaceNavigation(workspaceId: string, signal?: AbortSignal): Promise<WorkspaceActivation> {
  const result = await api<WorkspaceActivation>(`/workspaces/${workspaceId}/ui/contributions`, signal);
  return {
    navigation: result.navigation.slice().sort((a, b) => (a.order ?? 0) - (b.order ?? 0)),
    permissions: [...result.permissions].sort(),
    revision: result.revision,
  };
}

export function shellFixture(status: ShellViewModel['status']): ShellViewModel {
  const base: ShellViewModel = {
    status,
    workspaces: fixtureWorkspaces,
    activeWorkspaceId: fixtureWorkspaces[0].id,
    navigation: fixtureNavigation,
    permissions: [
      'contacts.organisations.custom-fields.read',
      'contacts.organisations.read',
      'contacts.organisations.write',
      'contacts.people.custom-fields.read',
      'contacts.people.read',
      'contacts.people.write',
      'contacts.settings.manage',
    ],
    activationRevision: 'fixture-contacts-enabled',
    user: fixtureUser,
  };
  if (status === 'empty') return {...base, navigation: [], permissions: [], message: 'No features are enabled for this workspace yet.'};
  if (status === 'denied') return {...base, navigation: [], permissions: [], message: 'You do not have permission to view this workspace’s features.'};
  if (status === 'error') return {...base, navigation: [], permissions: [], message: 'Feature navigation could not be loaded.'};
  if (status === 'loading') return {...base, navigation: [], permissions: []};
  return base;
}
