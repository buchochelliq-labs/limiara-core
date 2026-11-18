import type {DraftsContribution} from '../drafts/types';

export type Workspace = {id: string; name: string};

export type NavigationContribution = {
  id: string;
  label: string;
  route: string;
  icon?: string;
  order?: number;
};

export type SessionUser = {
  id: string;
  displayName: string;
  role: string;
};

export type ShellStatus = 'loading' | 'ready' | 'empty' | 'denied' | 'error';

export type ShellViewModel = {
  status: ShellStatus;
  workspaces: Workspace[];
  activeWorkspaceId: string;
  navigation: NavigationContribution[];
  permissions: string[];
  activationRevision: string;
  user: SessionUser;
  message?: string;
  /** Where this host keeps record drafts (ADR 0036 §3); absent when it keeps none. */
  drafts?: DraftsContribution | null;
};
