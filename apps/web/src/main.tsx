import React, {useCallback, useEffect, useRef, useState} from 'react';
import {createRoot} from 'react-dom/client';
import {UiShowcase} from './showcase/UiShowcase';
import {AppShell} from './shell/AppShell';
import {fixtureUser, loadShellContext, loadWorkspaceNavigation, shellFixture, WorkspaceRequestSequence} from './shell/data';
import type {ShellStatus, ShellViewModel} from './shell/types';
import {ExternalHostApp, ExternalUnavailableApp, isExternalRoutePath, parseExternalRoute} from './external-host/ExternalHostApp';
import './styles/global.css';

const debugStatus = import.meta.env.DEV ? new URLSearchParams(window.location.search).get('shell-state') as ShellStatus | null : null;
export const activationRevalidationIntervalMs = 15_000;

function LimiaraApp() {
  const [route, setRoute] = useState(window.location.pathname || '/');
  const [search, setSearch] = useState(window.location.search);
  const [model, setModel] = useState<ShellViewModel>(() => debugStatus ? shellFixture(debugStatus) : shellFixture('loading'));
  const modelRef = useRef(model);
  const [announcement, setAnnouncement] = useState('');
  const [reloadKey, setReloadKey] = useState(0);
  const requestSequence = useRef(new WorkspaceRequestSequence());
  const revalidationInFlight = useRef(false);

  const commitModel = useCallback((next: ShellViewModel) => {
    modelRef.current = next;
    setModel(next);
  }, []);

  const loadNavigation = useCallback(async (workspaceId: string, baseModel: Pick<ShellViewModel, 'workspaces' | 'user'>, announce = false) => {
    const sequence = requestSequence.current.begin();
    commitModel({status: 'loading', workspaces: baseModel.workspaces, activeWorkspaceId: workspaceId, navigation: [], permissions: [], activationRevision: '', user: baseModel.user});
    try {
      const {navigation, permissions, revision} = await loadWorkspaceNavigation(workspaceId);
      if (!requestSequence.current.isCurrent(sequence)) return;
      const workspace = baseModel.workspaces.find(item => item.id === workspaceId);
      commitModel({status: navigation.length ? 'ready' : 'empty', workspaces: baseModel.workspaces, activeWorkspaceId: workspaceId, navigation, permissions, activationRevision: revision, user: baseModel.user, message: navigation.length ? undefined : 'No features are enabled for this workspace yet.'});
      if (announce) setAnnouncement(`${workspace?.name ?? 'Workspace'} loaded. ${navigation.length ? `${navigation.length === 1 ? '1 feature' : `${navigation.length} features`} available.` : 'No features are enabled.'}`);
    } catch (reason) {
      if (!requestSequence.current.isCurrent(sequence)) return;
      const status = (reason as {status?: number}).status === 403 ? 'denied' : 'error';
      commitModel({status, workspaces: baseModel.workspaces, activeWorkspaceId: workspaceId, navigation: [], permissions: [], activationRevision: '', user: baseModel.user, message: status === 'denied' ? 'You do not have permission to view this workspace’s features.' : 'Feature navigation could not be loaded.'});
    }
  }, [commitModel]);

  const revalidateActivation = useCallback(async () => {
    const current = modelRef.current;
    if (!current.activeWorkspaceId || current.status === 'loading' || revalidationInFlight.current) return;
    const workspaceId = current.activeWorkspaceId;
    const sequence = requestSequence.current.begin();
    revalidationInFlight.current = true;
    try {
      const {navigation, permissions, revision} = await loadWorkspaceNavigation(workspaceId);
      if (!requestSequence.current.isCurrent(sequence)) return;
      const latest = modelRef.current;
      if (latest.activeWorkspaceId !== workspaceId || latest.activationRevision === revision) return;
      const workspace = latest.workspaces.find(item => item.id === workspaceId);
      commitModel({status: navigation.length ? 'ready' : 'empty', workspaces: latest.workspaces, activeWorkspaceId: workspaceId, navigation, permissions, activationRevision: revision, user: latest.user, message: navigation.length ? undefined : 'No features are enabled for this workspace yet.'});
      setAnnouncement(`${workspace?.name ?? 'Workspace'} permissions and features refreshed.`);
    } catch (reason) {
      if (!requestSequence.current.isCurrent(sequence)) return;
      const latest = modelRef.current;
      if (latest.activeWorkspaceId !== workspaceId) return;
      const status = (reason as {status?: number}).status === 403 ? 'denied' : 'error';
      commitModel({status, workspaces: latest.workspaces, activeWorkspaceId: workspaceId, navigation: [], permissions: [], activationRevision: '', user: latest.user, message: status === 'denied' ? 'You do not have permission to view this workspace’s features.' : 'Feature navigation could not be revalidated.'});
    } finally {
      revalidationInFlight.current = false;
    }
  }, [commitModel]);

  useEffect(() => {
    if (debugStatus) return;
    const controller = new AbortController();
    commitModel(shellFixture('loading'));
    loadShellContext(controller.signal).then(context => {
      if (controller.signal.aborted) return;
      const activeWorkspaceId = context.workspaces[0]?.id ?? '';
      if (!activeWorkspaceId) {
        commitModel({status: 'empty', workspaces: [], activeWorkspaceId: '', navigation: [], permissions: [], activationRevision: '', user: context.user, message: 'No workspaces are available for this account.'});
        return;
      }
      void loadNavigation(activeWorkspaceId, context);
    }).catch(reason => {
      if (controller.signal.aborted) return;
      commitModel({...shellFixture((reason as {status?: number}).status === 403 ? 'denied' : 'error'), workspaces: [], activeWorkspaceId: '', user: fixtureUser});
    });
    return () => controller.abort();
  }, [commitModel, loadNavigation, reloadKey]);

  useEffect(() => {
    if (debugStatus) return;
    const refresh = () => { void revalidateActivation(); };
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') refresh();
    };
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', refreshWhenVisible);
    const interval = window.setInterval(refresh, activationRevalidationIntervalMs);
    return () => {
      window.removeEventListener('focus', refresh);
      document.removeEventListener('visibilitychange', refreshWhenVisible);
      window.clearInterval(interval);
    };
  }, [revalidateActivation]);

  useEffect(() => {
    const syncRoute = () => {
      setRoute(window.location.pathname || '/');
      setSearch(window.location.search);
    };
    window.addEventListener('popstate', syncRoute);
    return () => window.removeEventListener('popstate', syncRoute);
  }, []);

  const navigate = (nextRoute: string) => {
    const next = new URL(nextRoute, window.location.origin);
    const current = `${window.location.pathname}${window.location.search}`;
    if (`${next.pathname}${next.search}` === current) return;
    window.history.pushState({}, '', nextRoute);
    setRoute(next.pathname || '/');
    setSearch(next.search);
  };

  const changeWorkspace = (workspaceId: string) => {
    if (debugStatus || workspaceId === model.activeWorkspaceId) return;
    setAnnouncement('Switching workspace. Previous feature data cleared.');
    void loadNavigation(workspaceId, model, true);
  };

  return <AppShell model={model} route={route} search={search} announcement={announcement} onNavigate={navigate} onWorkspaceChange={changeWorkspace} onRetry={() => setReloadKey(value => value + 1)} />;
}

const showDevelopmentShowcase = import.meta.env.DEV && window.location.pathname === '/__ui';
const externalRoute = parseExternalRoute(window.location.pathname);
const malformedExternalRoute = !externalRoute && isExternalRoutePath(window.location.pathname);
createRoot(document.getElementById('root')!).render(showDevelopmentShowcase ? <UiShowcase /> : externalRoute ? <ExternalHostApp route={externalRoute} /> : malformedExternalRoute ? <ExternalUnavailableApp host={window.location.pathname.startsWith('/portal') ? 'portal' : 'public-form'} /> : <LimiaraApp />);
