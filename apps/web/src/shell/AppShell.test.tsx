import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {activeContributionFor, AppShell, workspaceStateKey} from './AppShell';
import {shellFixture} from './data';

const noop = () => undefined;

function render(status: Parameters<typeof shellFixture>[0], route = '/contacts') {
  return renderToStaticMarkup(<AppShell model={shellFixture(status)} route={route} onNavigate={noop} onWorkspaceChange={noop} onRetry={noop} />);
}

function renderReadOnly(route: string) {
  const model = {
    ...shellFixture('ready'),
    permissions: ['contacts.people.read', 'contacts.organisations.read'],
    activationRevision: 'fixture-contacts-read-only',
  };
  return renderToStaticMarkup(<AppShell model={model} route={route} onNavigate={noop} onWorkspaceChange={noop} onRetry={noop} />);
}

describe('plugin-aware application shell', () => {
  it('opens kernel user records from any workspace, whatever features are enabled', () => {
    const markup = render('ready', '/users/usr-hannah');
    expect(markup).toContain('Workspace / Users');
    expect(markup).toContain('Loading user');
    expect(markup).not.toContain('Route unavailable');
    const readOnly = renderReadOnly('/users/usr-morgan');
    expect(readOnly).toContain('Loading user');
  });

  it('changes the route subtree identity for workspace and activation transitions', () => {
    const enabled = shellFixture('ready');
    const disabledRevision = {...enabled, activationRevision: 'contacts-disabled'};
    const otherWorkspace = {...enabled, activeWorkspaceId: enabled.workspaces[1].id};
    expect(workspaceStateKey(disabledRevision)).not.toBe(workspaceStateKey(enabled));
    expect(workspaceStateKey(otherWorkspace)).not.toBe(workspaceStateKey(enabled));
  });

  it('renders contribution navigation and identifies the active route', () => {
    const markup = render('ready');
    expect(markup).toContain('aria-label="Primary navigation"');
    expect(markup).toContain('href="/contacts"');
    expect(markup).toContain('aria-current="page"');
    expect(markup).toContain('Northstar Services application');
    expect(markup).toContain('Search people');
    expect(markup).toContain('Amelia Hart');
  });

  it('keeps shell landmarks present while features load', () => {
    const markup = render('loading');
    expect(markup).toContain('Loading workspace');
    expect(markup).toContain('aria-label="Loading workspace features"');
    expect(markup).toContain('aria-label="Primary navigation"');
  });

  it('explains an empty workspace without exposing plugin routes', () => {
    const markup = render('empty');
    expect(markup).toContain('No features enabled');
    expect(markup).not.toContain('href="/contacts"');
    expect(markup).not.toContain('Amelia Hart');
    expect(markup).not.toContain('platform.contacts');
  });

  it('explains permission denial without exposing unavailable routes', () => {
    const markup = render('denied');
    expect(markup).toContain('Workspace access is limited');
    expect(markup).toContain('Unavailable features and routes are not shown');
    expect(markup).not.toContain('href="/contacts"');
  });

  it('provides a recovery action after contribution failure', () => {
    const markup = render('error');
    expect(markup).toContain('Feature navigation is unavailable');
    expect(markup).toContain('Try again');
    expect(markup).not.toContain('href="/contacts"');
  });

  it('blocks direct navigation when the contribution is unavailable', () => {
    const markup = render('ready', '/campaigns');
    expect(markup).toContain('Page unavailable');
    expect(markup).toContain('This feature is disabled or you do not have permission to open it.');
  });

  it('renders the dedicated person creation route inside the shell', () => {
    const markup = render('ready', '/contacts/people/new');
    expect(markup).toContain('>Add person</h1>');
    expect(markup).toContain('Save person');
    expect(markup).toContain('Back to people');
  });

  it('renders a deep-linked person editing route inside the shell', () => {
    const markup = render('ready', '/contacts/people/maya-patel/edit');
    expect(markup).toContain('>Edit Maya Patel</h1>');
    expect(markup).toContain('Save changes');
    expect(markup).toContain('maya@northstar.example');
  });

  it('renders a deep-linked metadata person detail route inside the shell', () => {
    const markup = render('ready', '/contacts/people/maya-patel');
    expect(markup).toContain('data-page-id="platform.contacts.person-detail"');
    expect(markup).toContain('Maya Patel');
    expect(markup).toContain('Archive person');
  });

  it('renders metadata-driven organisation directory and detail routes', () => {
    const directory = render('ready', '/contacts/organisations');
    expect(directory).toContain('data-page-id="platform.contacts.organisation-directory"');
    expect(directory).toContain('Search organisations');
    const detail = render('ready', '/contacts/organisations/northstar');
    expect(detail).toContain('data-page-id="platform.contacts.organisation-detail"');
    expect(detail).toContain('Maya Patel');
  });

  it('propagates caller permissions into every Contacts route', () => {
    const people = renderReadOnly('/contacts');
    expect(people).toContain('Amelia Hart');
    expect(people).not.toContain('Add person');
    expect(people).not.toContain('Edit person');
    expect(people).not.toContain('Filter by relationship');

    const person = renderReadOnly('/contacts/people/maya-patel');
    expect(person).toContain('Maya Patel');
    expect(person).not.toContain('Edit person');
    expect(person).not.toContain('Archive person');

    const organisations = renderReadOnly('/contacts/organisations');
    expect(organisations).toContain('Northstar Services');
    expect(organisations).not.toContain('Add organisation');
    expect(organisations).not.toContain('Edit organisation');

    const organisation = renderReadOnly('/contacts/organisations/northstar');
    expect(organisation).toContain('Northstar Services');
    expect(organisation).not.toContain('Edit organisation');
    expect(organisation).not.toContain('Archive organisation');

    const directWriteRoute = renderReadOnly('/contacts/organisations/new');
    expect(directWriteRoute).toContain('You do not have permission to create or edit organisations.');
  });
});

describe('app launcher and feature routing', () => {
  it('lists every contributed feature in the launcher and the home page without hard-coding one', () => {
    const markup = render('ready', '/');
    expect(markup).toContain('aria-label="Feature launcher"');
    expect(markup).toContain('Your features');
    expect(markup).toContain('>People</strong>');
    expect(markup).toContain('>Organisations</strong>');
    expect(markup).toContain('2 features enabled');
    expect(markup).not.toContain('Contacts</strong>');
  });

  it('resolves nested routes to the feature with the longest matching route', () => {
    const navigation = [
      {id: 'contacts', label: 'People', route: '/contacts', order: 100},
      {id: 'contacts-organisations', label: 'Organisations', route: '/contacts/organisations', order: 110},
    ];
    expect(activeContributionFor(navigation, '/contacts/organisations/northstar')?.id).toBe('contacts-organisations');
    expect(activeContributionFor(navigation, '/contacts/people/maya-patel')?.id).toBe('contacts');
    expect(activeContributionFor(navigation, '/reports')).toBeUndefined();
    const markup = render('ready', '/contacts/organisations');
    expect(markup).toContain('href="/contacts/organisations" class="shell-nav__link" aria-current="page"');
    expect(markup).toContain('href="/contacts" class="shell-nav__link"><span');
  });

  it('uses feature wording everywhere a person reads the shell', () => {
    for (const status of ['ready', 'empty', 'denied', 'error', 'loading'] as const) {
      expect(render(status, '/').toLowerCase()).not.toMatch(/\bmodules?\b/);
    }
  });
});
