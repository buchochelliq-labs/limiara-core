import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {buildOrganisationDirectoryUrl, OrganisationDirectory, readOrganisationDirectoryLocation} from './OrganisationDirectory';
import {resolveContactsOrganisationDirectory, type OrganisationDirectoryQuery} from './organisation-metadata';

const noop = () => undefined;
const permissions = new Set(['contacts.organisations.read', 'contacts.organisations.write', 'contacts.organisations.custom-fields.read', 'contacts.people.read']);
const query: OrganisationDirectoryQuery = {query: '', status: 'All', sort: 'name', cursor: '', selected: ''};

describe('organisation directory URL state', () => {
  it('round-trips supported search, filter, sort, cursor, selection, and state values', () => {
    const url = buildOrganisationDirectoryUrl({query: 'north', status: 'Active', sort: 'recent', cursor: 'page-2', selected: 'northstar'}, 'archived');
    expect(url).toBe('/contacts/organisations?q=north&status=Active&sort=recent&cursor=page-2&selected=northstar&organisation-state=archived');
    expect(readOrganisationDirectoryLocation(url.slice(url.indexOf('?')))).toEqual({query: {query: 'north', status: 'Active', sort: 'recent', cursor: 'page-2', selected: 'northstar'}, state: 'archived'});
  });

  it('normalises unsupported URL values', () => {
    expect(readOrganisationDirectoryLocation('?status=secret&sort=wrong&cursor=offset&organisation-state=private')).toEqual({query, state: 'ready'});
  });
});

describe('metadata-driven organisation directory', () => {
  it('renders search, collection, selection preview, and organisation actions from metadata', () => {
    const markup = renderToStaticMarkup(<OrganisationDirectory search="?selected=northstar" onNavigate={noop} />);
    expect(markup).toContain('data-page-id="platform.contacts.organisation-directory"');
    expect(markup).toContain('Search organisations');
    expect(markup).toContain('Northstar Services');
    expect(markup).toContain('Open full record');
    expect(markup).toContain('Add organisation');
  });

  it('renders the same metadata contract in compact form', () => {
    const markup = renderToStaticMarkup(<OrganisationDirectory formFactor="compact" search="?selected=northstar" onNavigate={noop} />);
    expect(markup).toContain('metadata-renderer--compact');
    expect(markup).toContain('Northstar Services');
  });

  it('removes unconfigured custom fields and their filter', () => {
    const configured = new Set(['name', 'sector', 'telephone', 'address', 'website', 'updated_at', 'version']);
    const resolved = resolveContactsOrganisationDirectory({query: {...query, status: 'Active'}, permissions, configuredFields: configured});
    expect(JSON.stringify(resolved.page)).not.toContain('organisations-status-filter');
    expect(JSON.stringify(resolved.page)).not.toContain('account_status__c');
    expect(resolved.dataSources.directory.status).toBe('All');
  });

  it('makes stale selections and cursors explicit', () => {
    expect(resolveContactsOrganisationDirectory({query: {...query, selected: 'removed'}, permissions}).page.selected_variant).toBe('stale-selection');
    const stale = resolveContactsOrganisationDirectory({query: {...query, query: 'Northstar', cursor: 'page-2'}, permissions});
    expect(stale.page.selected_variant).toBe('stale-cursor');
    expect(JSON.stringify(stale.page.root)).toContain('This result page is no longer available');
  });

  it.each(['loading', 'empty', 'denied', 'error', 'disabled'] as const)('does not expose organisation records in the %s state', state => {
    const resolved = resolveContactsOrganisationDirectory({query, permissions, state, pluginEnabled: state !== 'disabled'});
    expect(JSON.stringify(resolved.dataSources)).not.toContain('Northstar Services');
    expect(JSON.stringify(resolved.page.root)).not.toContain('northstar.example');
  });

  it('removes write actions for a read-only caller', () => {
    const resolved = resolveContactsOrganisationDirectory({query: {...query, selected: 'northstar'}, permissions: new Set(['contacts.organisations.read'])});
    expect(JSON.stringify(resolved.page)).not.toContain('add-organisation');
    expect(JSON.stringify(resolved.page)).not.toContain('edit-organisation');
    expect(JSON.stringify(resolved.page)).toContain('open-organisation');
  });
});
