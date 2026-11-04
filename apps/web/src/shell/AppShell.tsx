import React, {useEffect, useMemo, useRef, useState} from 'react';
import {
  ArrowRight,
  Buildings,
  CaretDown,
  Gear,
  House,
  List,
  MagnifyingGlass,
  Question,
  SignOut,
  UserCircle,
  X,
} from '@phosphor-icons/react';
import {Alert, Avatar, Button, EmptyState, IconButton, Skeleton} from '../components/ui';
import {useResponsiveFormFactor} from '../hooks/useResponsiveFormFactor';
import {PeopleDirectory} from '../contacts/PeopleDirectory';
import {PersonForm} from '../contacts/PersonForm';
import {PersonDetail} from '../contacts/PersonDetail';
import {OrganisationDirectory} from '../contacts/OrganisationDirectory';
import {OrganisationDetail} from '../contacts/OrganisationDetail';
import type {ContactsClient} from '../contacts/api';
import {readForcedState, resolveContactsClient} from '../contacts/client';
import type {PeopleDirectoryState} from '../contacts/people-directory-metadata';
import type {PersonDetailState} from '../contacts/person-detail-metadata';
import brandMark from '../assets/limiara-mark.svg';
import {AppLauncher} from './AppLauncher';
import {activeContributionFor} from './navigation';
export {activeContributionFor} from './navigation';
import {ObjectGlyph, ObjectTile} from './ObjectIcon';
import type {NavigationContribution, ShellViewModel} from './types';
import './app-shell.css';
import {resolveUsersClient, type UsersClient} from '../users/api';
import {resolveDevicesClient, type DevicesClient} from '../devices/api';
import {DevicesPage, devicesRoute} from '../devices/DevicesPage';
import {UserRecordPage} from '../users/UserRecordPage';
import {resolveObjectsClient, type ObjectsClient} from '../objects/api';
import {NewRecordPage, ObjectHome, ObjectRecordPage} from '../objects/ObjectPages';
import {ObjectManager, ObjectManagerDetail, objectManagerRoute} from '../objects/ObjectManager';
import {MembersPage, membersRoute} from '../objects/MembersPage';
import {QueryConsole, queryRoute} from '../objects/QueryConsole';
import {ReportsPage, reportsRoute} from '../objects/Reports';
import {GlobalSearch, SearchResultsPage, searchRoute} from '../objects/GlobalSearch';

type AppShellProps = {
  model: ShellViewModel;
  route: string;
  search?: string;
  announcement?: string;
  onNavigate: (route: string) => void;
  onWorkspaceChange: (workspaceId: string) => void;
  onRetry: () => void;
  /** Contacts data access for the active workspace; resolved from the workspace when omitted. */
  contactsClient?: ContactsClient;
  /** Workspace user records; resolved from the workspace when omitted. */
  usersClient?: UsersClient;
  devicesClient?: DevicesClient;
  /** Workspace objects (ADR 0015); resolved from the workspace when omitted. */
  objectsClient?: ObjectsClient;
};

const writeStates = new Set(['ready', 'denied', 'error', 'duplicate', 'conflict']);
const peopleStates = new Set<PeopleDirectoryState>(['ready', 'loading', 'empty', 'denied', 'error', 'disabled', 'archived']);
const detailStates = new Set<PersonDetailState>(['ready', 'loading', 'missing', 'denied', 'error', 'archived', 'version-changed', 'disabled', 'workspace-changed']);

export function workspaceStateKey(model: Pick<ShellViewModel, 'activeWorkspaceId' | 'activationRevision'>) {
  return `${model.activeWorkspaceId}:${model.activationRevision}`;
}

function NavLink({label, route, active, icon, onNavigate}: {label: string; route: string; active: boolean; icon: React.ReactNode; onNavigate: (route: string) => void}) {
  return <a href={route} className="shell-nav__link" aria-current={active ? 'page' : undefined} onClick={event => { event.preventDefault(); onNavigate(route); }}>
    <span className="shell-nav__icon">{icon}</span>
    <span className="shell-nav__label">{label}</span>
  </a>;
}

function ShellNavigation({model, route, onNavigate, mobile = false}: Pick<AppShellProps, 'model' | 'route' | 'onNavigate'> & {mobile?: boolean}) {
  const active = activeContributionFor(model.navigation, route);
  return <nav className={mobile ? 'mobile-nav' : 'shell-nav'} aria-label="Primary navigation">
    {!mobile && <AppLauncher apps={model.navigation} route={route} onNavigate={onNavigate} />}
    <NavLink label="Home" route="/" active={route === '/'} icon={<House aria-hidden="true" />} onNavigate={onNavigate} />
    {model.navigation.map(item => <NavLink key={item.id} label={item.label} route={item.route} active={active?.id === item.id} icon={<ObjectGlyph icon={item.icon} />} onNavigate={onNavigate} />)}
    {mobile
      ? <NavLink label="Settings" route="/settings" active={route === '/settings' || route.startsWith('/settings/')} icon={<Gear aria-hidden="true" />} onNavigate={onNavigate} />
      : <div className="shell-nav__bottom"><NavLink label="Settings" route="/settings" active={route === '/settings' || route.startsWith('/settings/')} icon={<Gear aria-hidden="true" />} onNavigate={onNavigate} /></div>}
  </nav>;
}

function greeting(now: Date) {
  const hour = now.getHours();
  return hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
}

/** The workspace home: the apps the workspace has enabled, each opening its own destination. */
function WorkspaceHome({model, onNavigate, headingRef}: Pick<AppShellProps, 'model' | 'onNavigate'> & {headingRef: React.RefObject<HTMLHeadingElement | null>}) {
  const workspace = model.workspaces.find(item => item.id === model.activeWorkspaceId);
  const now = new Date();
  return <div className="content-canvas workspace-home">
    <header className="page-header">
      <div>
        <p className="page-header__breadcrumb">{now.toLocaleDateString('en-GB', {weekday: 'long', day: 'numeric', month: 'long'})}</p>
        <h1 ref={headingRef} tabIndex={-1}>{greeting(now)}, {model.user.displayName.split(' ')[0]}</h1>
        <p>{model.navigation.length === 1 ? '1 feature enabled' : `${model.navigation.length} features enabled`} in {workspace?.name ?? 'this workspace'}.</p>
      </div>
    </header>
    <section className="home-apps" aria-labelledby="home-apps-heading">
      <h2 id="home-apps-heading">Your features</h2>
      <ul>
        {model.navigation.map(item => <li key={item.id}>
          <a href={item.route} className="home-app" onClick={event => { event.preventDefault(); onNavigate(item.route); }}>
            <ObjectTile item={item} size="large" />
            <span className="home-app__text"><strong>{item.label}</strong><span>Open {item.label.toLowerCase()}</span></span>
            <ArrowRight aria-hidden="true" />
          </a>
        </li>)}
      </ul>
      <p className="home-apps__note">Cards from your features (recent records, counts and tasks) will appear here as features contribute them.</p>
    </section>
  </div>;
}

function WorkspaceControl({model, onWorkspaceChange, compact = false}: Pick<AppShellProps, 'model' | 'onWorkspaceChange'> & {compact?: boolean}) {
  const workspace = model.workspaces.find(item => item.id === model.activeWorkspaceId);
  return <label className={compact ? 'workspace-control workspace-control--compact' : 'workspace-control'}>
    <span className="sr-only">Active workspace</span>
    {!compact && <span className="workspace-control__icon"><Buildings aria-hidden="true" weight="fill" /></span>}
    <select value={model.activeWorkspaceId} onChange={event => onWorkspaceChange(event.target.value)} disabled={model.status === 'loading' || model.workspaces.length === 0}>
      {model.workspaces.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
    </select>
    <CaretDown aria-hidden="true" />
  </label>;
}

function UserMenu({model}: Pick<AppShellProps, 'model'>) {
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setOpen(false); buttonRef.current?.focus(); }
    };
    document.addEventListener('keydown', close);
    return () => document.removeEventListener('keydown', close);
  }, [open]);

  return <div className="user-menu">
    <button ref={buttonRef} className="user-menu__trigger" aria-expanded={open} aria-haspopup="menu" onClick={() => setOpen(value => !value)}>
      <Avatar name={model.user.displayName} size="small" />
      <span className="user-menu__identity"><strong>{model.user.displayName}</strong><small>{model.user.role}</small></span>
      <CaretDown aria-hidden="true" />
    </button>
    {open && <div className="user-menu__popover" aria-label="Account menu">
      <div className="user-menu__summary"><Avatar name={model.user.displayName} /><div><strong>{model.user.displayName}</strong><span>{model.user.role}</span></div></div>
      <button><UserCircle aria-hidden="true" />Account settings</button>
      <button disabled><SignOut aria-hidden="true" />Sign out</button>
    </div>}
  </div>;
}

function ShellState({model, onRetry}: Pick<AppShellProps, 'model' | 'onRetry'>) {
  if (model.status === 'loading') return <section className="shell-state" aria-label="Loading workspace features"><Skeleton label="Loading workspace features" lines={3} /><Skeleton label="Loading feature navigation" lines={2} /></section>;
  if (model.status === 'empty') return <EmptyState title="No features enabled" description={model.message ?? 'This workspace does not have any features enabled yet.'} action={<Button size="small" variant="secondary">Browse available features</Button>} />;
  if (model.status === 'denied') return <div className="shell-state"><Alert variant="warning" title="Workspace access is limited">{model.message}</Alert><p>Ask a workspace administrator to review your role. Unavailable features and routes are not shown.</p></div>;
  if (model.status === 'error') return <div className="shell-state"><Alert variant="danger" title="Feature navigation is unavailable">{model.message}</Alert><Button variant="secondary" onClick={onRetry}>Try again</Button></div>;
  return null;
}

function SettingsHome({onNavigate, headingRef}: Pick<AppShellProps, 'onNavigate'> & {headingRef: React.RefObject<HTMLHeadingElement | null>}) {
  return <div className="content-canvas">
    <header className="page-header"><div><p className="page-header__breadcrumb">Workspace</p><h1 ref={headingRef} tabIndex={-1}>Settings</h1></div></header>
    <ul className="settings-links">
      <li><a href={objectManagerRoute} onClick={event => { event.preventDefault(); onNavigate(objectManagerRoute); }}><strong>Object Manager</strong><span>Objects, fields, record access and access by role</span></a></li>
      <li><a href={membersRoute} onClick={event => { event.preventDefault(); onNavigate(membersRoute); }}><strong>Members</strong><span>Who reports to whom, for record access</span></a></li>
      <li><a href={devicesRoute} onClick={event => { event.preventDefault(); onNavigate(devicesRoute); }}><strong>Devices</strong><span>Phones and computers signed in with the app, and revoking them</span></a></li>
      <li><a href={queryRoute} onClick={event => { event.preventDefault(); onNavigate(queryRoute); }}><strong>Query</strong><span>Query records of any object with the query language</span></a></li>
    </ul>
  </div>;
}

function PageContent({route, search = '', model, onNavigate, contactsClient, usersClient, devicesClient, objectsClient}: Pick<AppShellProps, 'route' | 'search' | 'model' | 'onNavigate' | 'contactsClient' | 'usersClient' | 'devicesClient' | 'objectsClient'>) {
  const callerPermissions = useMemo(() => new Set(model.permissions), [model.permissions]);
  // The client is recreated only when the workspace or a forced write state
  // changes; other query values must not discard its caches.
  const forcedWriteState = readForcedState(search, 'form-state', writeStates) ?? '';
  const resolvedClient = useMemo(() => contactsClient ?? resolveContactsClient(model.activeWorkspaceId, forcedWriteState ? `?form-state=${forcedWriteState}` : ''), [contactsClient, forcedWriteState, model.activeWorkspaceId]);
  const resolvedUsers = useMemo(() => usersClient ?? resolveUsersClient(model.activeWorkspaceId), [usersClient, model.activeWorkspaceId]);
  const resolvedDevices = useMemo(() => devicesClient ?? resolveDevicesClient(model.activeWorkspaceId), [devicesClient, model.activeWorkspaceId]);
  const resolvedObjects = useMemo(() => objectsClient ?? resolveObjectsClient(model.activeWorkspaceId), [objectsClient, model.activeWorkspaceId]);
  const forcedPeopleState = readForcedState(search, 'people-state', peopleStates);
  const forcedDetailState = readForcedState(search, 'detail-state', detailStates);
  const activeContribution = activeContributionFor(model.navigation, route);
  // Contacts routes are still matched here until pages register their own
  // routes; any Contacts-owned app being active unlocks them.
  const contactsActive = activeContribution !== undefined && activeContribution.id.startsWith('contacts');
  const isHome = route === '/';
  // User records belong to the kernel, so any member can open them whatever
  // features are enabled (ADR 0014).
  const userId = route.match(/^\/users\/([^/]+)$/)?.[1];
  // Engine objects and the Object Manager are kernel pages: the engine
  // decides access, and every object answers 404 to callers who cannot read
  // it, whether or not it is in navigation (ADR 0015).
  const objectMatch = route.match(/^\/o\/([^/]+)(?:\/([^/]+))?$/);
  const objectApiName = objectMatch ? decodeRouteSegment(objectMatch[1] ?? '') : undefined;
  const objectRecordId = objectMatch?.[2] ? decodeRouteSegment(objectMatch[2]) : undefined;
  const managedObject = route.match(/^\/settings\/objects\/([^/]+)$/)?.[1];
  const editPersonId = route.match(/^\/contacts\/people\/([^/]+)\/edit$/)?.[1];
  const detailPersonId = route.match(/^\/contacts\/people\/([^/]+)$/)?.[1];
  const editOrganisationId = route.match(/^\/contacts\/organisations\/([^/]+)\/edit$/)?.[1];
  const detailOrganisationId = route.match(/^\/contacts\/organisations\/([^/]+)$/)?.[1];
  const editPersonName = editPersonId ? editPersonId.split('-').map(part => `${part[0]?.toUpperCase() ?? ''}${part.slice(1)}`).join(' ') : '';
  const detailPersonName = detailPersonId ? detailPersonId.split('-').map(part => `${part[0]?.toUpperCase() ?? ''}${part.slice(1)}`).join(' ') : '';
  const organisationName = (editOrganisationId ?? detailOrganisationId)?.split('-').map(part => `${part[0]?.toUpperCase() ?? ''}${part.slice(1)}`).join(' ') ?? '';
  const title = isHome ? 'Workspace overview' : userId ? 'User' : route === '/settings' ? 'Settings' : route === objectManagerRoute || managedObject ? 'Object Manager' : route === membersRoute ? 'Members' : route === devicesRoute ? 'Devices' : route === queryRoute ? 'Query' : route === reportsRoute || route.startsWith(`${reportsRoute}/`) ? 'Reports' : route === searchRoute ? 'Search' : objectApiName ? (activeContribution?.label ?? 'Records') : route === '/contacts/people/new' ? 'Add person' : route === '/contacts/organisations/new' ? 'Add organisation' : editPersonId ? `Edit ${editPersonName}` : detailPersonId ? detailPersonName : editOrganisationId ? `Edit ${organisationName}` : detailOrganisationId ? organisationName : route === '/contacts/organisations' ? 'Organisations' : activeContribution?.label ?? 'Page unavailable';
  const subtitle = activeContribution ? `Manage ${activeContribution.label.toLowerCase()} across this workspace.` : 'This route is not available in the current workspace.';
  const headingRef = useRef<HTMLHeadingElement>(null);
  const hasMounted = useRef(false);

  useEffect(() => {
    document.title = `${title} - Limiara`;
    if (hasMounted.current) headingRef.current?.focus({preventScroll: true});
    hasMounted.current = true;
  }, [title, model.activeWorkspaceId]);

  if (route === '/settings') return <SettingsHome onNavigate={onNavigate} headingRef={headingRef} />;
  if (route === membersRoute) return <MembersPage client={resolvedObjects} onNavigate={onNavigate} headingRef={headingRef} />;
  if (route === devicesRoute) return <DevicesPage client={resolvedDevices} onNavigate={onNavigate} headingRef={headingRef} />;
  if (route === queryRoute) return <QueryConsole client={resolvedObjects} onNavigate={onNavigate} headingRef={headingRef} />;
  if (route === reportsRoute || route.startsWith(`${reportsRoute}/`)) return <ReportsPage client={resolvedObjects} route={route} onNavigate={onNavigate} headingRef={headingRef} />;
  if (route === searchRoute) return <SearchResultsPage client={resolvedObjects} search={search} onNavigate={onNavigate} headingRef={headingRef} />;
  if (route === objectManagerRoute) return <ObjectManager client={resolvedObjects} onNavigate={onNavigate} headingRef={headingRef} />;
  if (managedObject) return <ObjectManagerDetail key={managedObject} client={resolvedObjects} apiName={decodeRouteSegment(managedObject)} onNavigate={onNavigate} headingRef={headingRef} />;
  if (objectApiName && objectRecordId === 'new') return <NewRecordPage key={objectApiName} client={resolvedObjects} apiName={objectApiName} onNavigate={onNavigate} headingRef={headingRef} />;
  if (objectApiName && objectRecordId) return <ObjectRecordPage key={`${objectApiName}/${objectRecordId}`} client={resolvedObjects} apiName={objectApiName} recordId={objectRecordId} onNavigate={onNavigate} headingRef={headingRef} />;
  if (objectApiName) return <ObjectHome key={objectApiName} client={resolvedObjects} apiName={objectApiName} onNavigate={onNavigate} headingRef={headingRef} />;
  if (userId) return <UserRecordPage key={userId} userId={decodeRouteSegment(userId)} client={resolvedUsers} headingRef={headingRef} onNavigate={onNavigate} onTitle={name => { document.title = `${name} - Limiara`; }} />;
  if (route === '/contacts/people/new' && contactsActive) return <PersonForm client={resolvedClient} onNavigate={onNavigate} permissions={callerPermissions} />;
  if (editPersonId && contactsActive) return <PersonForm key={editPersonId} client={resolvedClient} personId={editPersonId} onNavigate={onNavigate} permissions={callerPermissions} />;
  if (detailPersonId && contactsActive) return <PersonDetail key={detailPersonId} client={resolvedClient} personId={detailPersonId} workspaceId={model.activeWorkspaceId} onNavigate={onNavigate} permissions={callerPermissions} forcedState={forcedDetailState} />;
  if ((route === '/contacts/organisations/new' || editOrganisationId) && contactsActive) {
    if (!callerPermissions.has('contacts.organisations.write')) return <div className="content-canvas"><Alert variant="warning" title="Route unavailable">You do not have permission to create or edit organisations.</Alert></div>;
    return <div className="content-canvas"><header className="page-header"><div><p className="page-header__breadcrumb">Contacts / Organisations</p><h1 tabIndex={-1}>{title}</h1></div></header><Alert variant="info" title="Organisation form route ready">The create and edit form will bind to this route when the organisation form metadata is delivered.</Alert><Button variant="secondary" onClick={() => onNavigate('/contacts/organisations')}>Back to organisations</Button></div>;
  }
  if (detailOrganisationId && contactsActive) return <OrganisationDetail key={detailOrganisationId} organisationId={detailOrganisationId} workspaceId={model.activeWorkspaceId} onNavigate={onNavigate} callerPermissions={callerPermissions} />;
  if (route === '/contacts/organisations' && contactsActive) return <OrganisationDirectory search={search} onNavigate={onNavigate} callerPermissions={callerPermissions} />;
  if (route === '/contacts' && contactsActive) return <PeopleDirectory route={route} search={search} client={resolvedClient} onNavigate={onNavigate} permissions={callerPermissions} forcedState={forcedPeopleState} />;

  if (isHome) return <WorkspaceHome model={model} onNavigate={onNavigate} headingRef={headingRef} />;
  if (!activeContribution) return <div className="content-canvas"><header className="page-header"><div><p className="page-header__breadcrumb">Workspace</p><h1 ref={headingRef} tabIndex={-1}>{title}</h1><p>{subtitle}</p></div></header><Alert variant="warning" title="Route unavailable">This feature is disabled or you do not have permission to open it.</Alert></div>;

  return <div className="content-canvas">
    <header className="page-header">
      <div><p className="page-header__breadcrumb">Features / {activeContribution.label}</p><h1 ref={headingRef} tabIndex={-1}>{title}</h1><p>{subtitle}</p></div>
    </header>
    <section className="module-ready" aria-labelledby="module-ready-heading"><div className="module-ready__icon"><ObjectGlyph icon={activeContribution.icon} /></div><div><p className="app-eyebrow">Feature enabled</p><h2 id="module-ready-heading">{activeContribution.label} is ready</h2><p>The application shell has loaded this destination from the feature's contribution. Lists and records will appear here without changing the surrounding workspace context.</p></div></section>
  </div>;
}

function decodeRouteSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

export function AppShell({model, route, search, announcement, onNavigate, onWorkspaceChange, onRetry, contactsClient, usersClient, devicesClient, objectsClient}: AppShellProps) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const shellFormFactor = useResponsiveFormFactor();
  const drawerRef = useRef<HTMLDivElement>(null);
  const mainRef = useRef<HTMLElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const focusedWorkspaceRef = useRef(model.activeWorkspaceId);
  const activeWorkspace = model.workspaces.find(item => item.id === model.activeWorkspaceId);

  useEffect(() => {
    if (!drawerOpen) return;
    drawerRef.current?.querySelector<HTMLElement>('a, button, select')?.focus();
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setDrawerOpen(false); menuButtonRef.current?.focus(); }
      if (event.key === 'Tab' && drawerRef.current) {
        const focusable = Array.from(drawerRef.current.querySelectorAll<HTMLElement>('a[href], button:not(:disabled), select:not(:disabled)'));
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (!first || !last) return;
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
    };
    document.addEventListener('keydown', close);
    return () => document.removeEventListener('keydown', close);
  }, [drawerOpen]);

  useEffect(() => {
    if (shellFormFactor === 'wide') setDrawerOpen(false);
  }, [shellFormFactor]);

  useEffect(() => {
    if (model.status !== 'ready' || focusedWorkspaceRef.current === model.activeWorkspaceId) return;
    focusedWorkspaceRef.current = model.activeWorkspaceId;
    mainRef.current?.querySelector<HTMLHeadingElement>('h1')?.focus({preventScroll: true});
  }, [model.activeWorkspaceId, model.status]);

  const navigate = (nextRoute: string) => { setDrawerOpen(false); onNavigate(nextRoute); };
  const shellLabel = useMemo(() => `${activeWorkspace?.name ?? 'Limiara'} application`, [activeWorkspace]);
  const searchClient = useMemo(() => objectsClient ?? resolveObjectsClient(model.activeWorkspaceId), [objectsClient, model.activeWorkspaceId]);

  return <div className="shell" aria-label={shellLabel}>
    <a className="skip-link" href="#main-content">Skip to main content</a>
    <div className="sr-only" aria-live="polite" aria-atomic="true">{announcement}</div>

    <aside className="shell-rail">
      <a className="brand" href="/" aria-label="Limiara home" onClick={event => { event.preventDefault(); navigate('/'); }}><img src={brandMark} alt="" /><span>Limiara</span></a>
      <ShellNavigation model={model} route={route} onNavigate={navigate} />
    </aside>

    <header className="shell-topbar">
      <button ref={menuButtonRef} className="mobile-menu-button" aria-label="Open navigation" aria-expanded={drawerOpen} aria-controls="mobile-navigation" onClick={() => setDrawerOpen(true)}><List aria-hidden="true" /></button>
      <WorkspaceControl model={model} onWorkspaceChange={onWorkspaceChange} />
      {model.status === 'ready' ? <GlobalSearch key={model.activeWorkspaceId} client={searchClient} onNavigate={navigate} /> : <label className="global-search"><MagnifyingGlass aria-hidden="true" /><span className="sr-only">Search Limiara</span><input type="search" placeholder="Search Limiara" disabled /><kbd>⌘ K</kbd></label>}
      <div className="topbar-actions"><IconButton label="Help" variant="quiet" icon={<Question aria-hidden="true" />} /><UserMenu model={model} /></div>
    </header>

    {drawerOpen && <div className="mobile-drawer-layer"><button className="mobile-drawer-backdrop" aria-label="Close navigation" onClick={() => { setDrawerOpen(false); menuButtonRef.current?.focus(); }} /><div ref={drawerRef} className="mobile-drawer" id="mobile-navigation" role="dialog" aria-modal="true" aria-label="Navigation menu"><div className="mobile-drawer__header"><a className="brand" href="/" onClick={event => { event.preventDefault(); navigate('/'); }}><img src={brandMark} alt="" /><span>Limiara</span></a><IconButton label="Close navigation" variant="quiet" icon={<X aria-hidden="true" />} onClick={() => { setDrawerOpen(false); menuButtonRef.current?.focus(); }} /></div><WorkspaceControl compact model={model} onWorkspaceChange={onWorkspaceChange} /><ShellNavigation mobile model={model} route={route} onNavigate={navigate} /></div></div>}

    <main ref={mainRef} id="main-content" className="shell-main">
      {model.status === 'ready' ? <PageContent key={workspaceStateKey(model)} route={route} search={search} model={model} onNavigate={navigate} contactsClient={contactsClient} usersClient={usersClient} devicesClient={devicesClient} objectsClient={objectsClient} /> : <div className="content-canvas"><header className="page-header"><div><p className="page-header__breadcrumb">Workspace</p><h1 tabIndex={-1}>{model.status === 'loading' ? 'Loading workspace' : 'Workspace features'}</h1></div></header><ShellState model={model} onRetry={onRetry} /></div>}
    </main>
  </div>;
}
