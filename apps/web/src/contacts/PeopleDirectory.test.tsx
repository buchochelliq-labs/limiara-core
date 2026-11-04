import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import type {PeoplePage} from './api';
import {createFixtureContactsClient, fixturePeoplePage, fixtureRecord} from './fixture-client';
import {people} from './people-data';
import {PeopleDirectory, buildPeopleDirectoryUrl, readPeopleDirectoryLocation} from './PeopleDirectory';
import {resolveContactsPeopleDirectory, type PeopleDirectoryQuery, type PeopleDirectoryState} from './people-directory-metadata';
import {contactsPersonPermissions} from './person-metadata';
import {loyaltyDefaultSet} from '../plugin-ui/extension-fields/fixtures';

const noop = () => undefined;
const allPermissions = new Set<string>(Object.values(contactsPersonPermissions));
const readOnlyPermissions = new Set<string>([contactsPersonPermissions.read]);
const defaultQuery: PeopleDirectoryQuery = {query: '', label: 'All', sort: 'name', cursor: '', selected: ''};
const client = createFixtureContactsClient();

type ResolveOptions = Parameters<typeof resolveContactsPeopleDirectory>[0];

/** Resolves against the fixture catalogue the way the component does once its page has loaded. */
function resolve(options: Omit<ResolveOptions, 'page' | 'selected'> & {page?: PeoplePage}) {
  const page = options.page ?? fixturePeoplePage({search: options.query.query, label: options.query.label, sort: options.query.sort, status: 'active', cursor: options.query.cursor});
  const selectedId = options.selectedId ?? options.query.selected;
  const person = people.find(item => item.id === selectedId);
  return resolveContactsPeopleDirectory({...options, page, selected: person ? fixtureRecord(person, options.state === 'archived') : undefined});
}

describe('people directory metadata migration', () => {
  it('renders the Contacts-owned collection page through generic host components', () => {
    const markup = renderToStaticMarkup(<PeopleDirectory route="/contacts" client={client} onNavigate={noop} />);
    expect(markup).toContain('data-page-id="platform.contacts.people-directory"');
    expect(markup).toContain('data-metadata-node="people-collection"');
    expect(markup).toContain('Search people');
    expect(markup).toContain('Filter by relationship');
    expect(markup).toContain('Sort people');
    expect(markup).toContain('Amelia Hart');
    expect(markup).toContain('Ashdown Studio');
    expect(markup).toContain('Showing 6 of 8');
    expect(markup).not.toContain('person-row__target');
    // The default presentation is a captioned table whose first column names each row.
    expect(markup).toContain('meta-collection--table');
    expect(markup).toContain('<th scope="row">');
    expect(markup).toContain('aria-sort="ascending"');
    expect(markup).toContain('Open<span class="sr-only">: Amelia Hart</span>');
    // Newest-first ordering is announced as descending, matching the API request.
    const recent = renderToStaticMarkup(<PeopleDirectory route="/contacts" search="?sort=recent" client={client} onNavigate={noop} />);
    expect(recent).toContain('aria-sort="descending"');
    expect(recent).not.toContain('aria-sort="ascending"');
    const cards = renderToStaticMarkup(<PeopleDirectory route="/contacts" search="?view=cards" client={client} onNavigate={noop} />);
    expect(cards).toContain('meta-collection--cards');
    expect(cards).not.toContain('<table');
  });

  it('shows active filters as removable chips', () => {
    const resolved = resolve({query: {...defaultQuery, query: 'Maya', label: 'Client'}, permissions: allPermissions});
    expect(resolved.dataSources.directory.chips).toEqual([
      {id: 'query', label: 'Search: Maya', action: 'search-people', parameter: 'query', value: ''},
      {id: 'label', label: 'Relationship: Client', action: 'filter-people', parameter: 'label', value: 'All'},
    ]);
    const markup = renderToStaticMarkup(<PeopleDirectory route="/contacts" search="?q=Maya&label=Client" client={client} onNavigate={noop} />);
    expect(markup).toContain('aria-label="Remove Search: Maya"');
    expect(markup).toContain('aria-label="Remove Relationship: Client"');
    expect(renderToStaticMarkup(<PeopleDirectory route="/contacts" client={client} onNavigate={noop} />)).not.toContain('meta-filter-chips');
  });

  it('renders an accessible selected-person preview for a deep link', () => {
    const markup = renderToStaticMarkup(<PeopleDirectory route="/contacts" search="?selected=maya-patel" client={client} onNavigate={noop} />);
    expect(markup).toContain('aria-label="Maya Patel preview"');
    expect(markup).toContain('mailto:maya@northstar.example');
    expect(markup).toContain('tel:+442079460271');
    expect(markup).toContain('Open full record');
    expect(markup).toContain('aria-current="true"');
    expect(markup).toContain('tabindex="-1"');
  });

  it('round-trips only supported query, filter, sort, cursor, and state values', () => {
    expect(readPeopleDirectoryLocation('?q=Maya&label=Client&sort=recent&cursor=page-2&secret=x')).toEqual({
      query: {query: 'Maya', label: 'Client', sort: 'recent', cursor: 'page-2', selected: ''},
      state: 'ready',
    });
    expect(buildPeopleDirectoryUrl('/contacts', {query: 'Maya', label: 'Client', sort: 'recent', cursor: 'page-2', selected: 'maya-patel'})).toBe('/contacts?q=Maya&label=Client&sort=recent&cursor=page-2&selected=maya-patel');
    expect(readPeopleDirectoryLocation('?view=cards').query.view).toBe('cards');
    expect(readPeopleDirectoryLocation('?view=table').query.view).toBeUndefined();
    expect(buildPeopleDirectoryUrl('/contacts', {...defaultQuery, view: 'cards'})).toBe('/contacts?view=cards');
    expect(readPeopleDirectoryLocation('?label=PrivateField&sort=unknown&people-state=unknown')).toEqual({query: defaultQuery, state: 'ready'});
    // Cursors are opaque API tokens; only their length is bounded.
    expect(readPeopleDirectoryLocation('?cursor=opaque-token').query.cursor).toBe('opaque-token');
    expect(readPeopleDirectoryLocation(`?cursor=${'x'.repeat(600)}`).query.cursor).toBe('');
  });

  it('never emits hidden or unconfigured custom fields in resolved metadata or data', () => {
    const standardOnly = resolve({query: {...defaultQuery, label: 'Client'}, selectedId: 'maya-patel', permissions: readOnlyPermissions});
    const encoded = JSON.stringify(standardOnly);
    expect(encoded).not.toContain('relationship_type__c');
    expect(encoded).not.toContain('service_tier__c');
    expect(encoded).not.toContain('people-relationship-filter');
    expect(standardOnly.dataSources.directory.label).toBe('All');
    const standardMarkup = renderToStaticMarkup(<PeopleDirectory route="/contacts/people/maya-patel" client={client} onNavigate={noop} permissions={readOnlyPermissions} />);
    expect(standardMarkup).not.toContain('people-relationship-filter');
    expect(standardMarkup).not.toContain('relationship_type__c');

    const configured = new Set(['full_name', 'email', 'job_title']);
    const configuredOnly = resolve({query: defaultQuery, selectedId: 'maya-patel', permissions: allPermissions, configuredFields: configured});
    expect(JSON.stringify(configuredOnly)).not.toContain('telephone');
    expect(JSON.stringify(configuredOnly)).not.toContain('organisation_id');
    expect(JSON.stringify(configuredOnly)).not.toContain('relationship_type__c');
  });

  it.each([
    ['loading', 'loading', 'Loading people'],
    ['empty', 'empty', 'Add your first person'],
    ['error', 'error', 'People could not be loaded'],
    ['denied', 'denied', 'Content unavailable'],
    ['disabled', 'plugin-disabled', 'Contacts is disabled'],
  ] as Array<[PeopleDirectoryState, string, string]>)('renders the %s state without leaking records', (state, variant, message) => {
    const resolved = resolve({query: defaultQuery, selectedId: 'maya-patel', state, permissions: allPermissions, pluginEnabled: state !== 'disabled'});
    expect(resolved.page.selected_variant).toBe(variant);
    const markup = renderToStaticMarkup(<PeopleDirectory route="/contacts/people/maya-patel" client={client} onNavigate={noop} pluginEnabled={state !== 'disabled'} forcedState={state} />);
    expect(JSON.stringify(resolved)).not.toContain('maya@northstar.example');
    expect(markup).not.toContain('maya@northstar.example');
    expect(JSON.stringify(resolved.page.root)).toContain(state === 'denied' ? 'people-denied' : state === 'disabled' ? 'contacts-disabled' : message);
  });

  it('models no-results, stale-selection, archived-selection, and cursor pages explicitly', () => {
    const noResults = resolve({query: {...defaultQuery, query: 'nobody'}, permissions: allPermissions});
    expect(noResults.page.selected_variant).toBe('no-results');
    expect(JSON.stringify(noResults.page.root)).toContain('people-no-results');

    const stale = resolve({query: defaultQuery, selectedId: 'missing-person', permissions: allPermissions});
    expect(stale.page.selected_variant).toBe('stale-selection');
    expect(JSON.stringify(stale.page.root)).toContain('stale-person-selection');

    const pending = resolve({query: defaultQuery, selectedId: 'missing-person', selectedPending: true, permissions: allPermissions});
    expect(pending.page.selected_variant).toBe('selection-loading');
    expect(JSON.stringify(pending.page.root)).toContain('pending-person-selection');

    const archived = resolve({query: defaultQuery, selectedId: 'maya-patel', state: 'archived', permissions: allPermissions});
    expect(archived.page.selected_variant).toBe('archived-selection');
    expect(JSON.stringify(archived.dataSources)).toContain('"archived":true');
    expect(JSON.stringify(archived.page.root)).not.toContain('edit-selected-person');

    const secondPage = resolve({query: {...defaultQuery, cursor: 'page-2'}, permissions: allPermissions});
    expect(secondPage.dataSources.directory.previous_cursor).toBe('page-1');
    expect(secondPage.dataSources.directory.next_cursor).toBe('');
    expect(secondPage.dataSources.directory.result_description).toBe('Showing 2 of 8');

    const empty = resolve({query: defaultQuery, page: {records: [], nextCursor: ''}, permissions: allPermissions});
    expect(empty.page.selected_variant).toBe('empty');
    expect(JSON.stringify(empty.page.root)).toContain('Add your first person');
  });

  it('describes API pages without a total through opaque cursors', () => {
    const page: PeoplePage = {records: fixturePeoplePage({search: '', label: 'All', sort: 'name', status: 'active', cursor: ''}).records, nextCursor: 'opaque-next'};
    const first = resolveContactsPeopleDirectory({query: defaultQuery, page, previousCursor: '', labelFilter: false, permissions: allPermissions});
    expect(first.dataSources.directory.result_heading).toBe('6 people on this page');
    expect(first.dataSources.directory.result_description).toBe('Showing 6, more available');
    expect(first.dataSources.directory.previous_cursor).toBe('');
    expect(first.dataSources.directory.next_cursor).toBe('opaque-next');
    expect(JSON.stringify(first.page.root)).not.toContain('people-relationship-filter');
    expect(first.page.actions.map(action => action.id)).not.toContain('filter-people');

    const second = resolveContactsPeopleDirectory({query: {...defaultQuery, cursor: 'opaque-next'}, page: {records: page.records.slice(0, 2), nextCursor: ''}, previousCursor: 'page-1', labelFilter: false, permissions: allPermissions});
    expect(second.dataSources.directory.previous_cursor).toBe('page-1');
    expect(second.dataSources.directory.result_description).toBe('Showing 2');
  });

  it('shows option labels rather than raw choice values for configured custom fields', () => {
    const resolved = resolve({query: defaultQuery, selectedId: 'maya-patel', permissions: allPermissions});
    const selected = resolved.dataSources.directory.selected as {values: Record<string, unknown>};
    expect(selected.values.relationship_type__c).toBe('Client');
    expect(selected.values.service_tier__c).toBe('Priority');
    expect(selected.values.tags).toBe('operations, priority');
  });
});

describe('fields other plugins add to people, as list columns', () => {
  it('appear as columns only where an administrator placed them', () => {
    const page = fixturePeoplePage({search: '', label: 'All', sort: 'name', status: 'active', cursor: ''});
    const withValues = {...page, records: page.records.map(record => record.id === 'maya-patel' ? {...record, extensions: {'example.loyalty.tier': 'gold'}} : record)};
    const placed = resolveContactsPeopleDirectory({query: defaultQuery, page: withValues, permissions: allPermissions, extensions: {...loyaltyDefaultSet, placements: [{field_id: 'example.loyalty.tier', zone: 'list-columns', position: 1}]}});
    const collection = JSON.stringify(placed.page.root);
    expect(collection).toContain('"key":"example.loyalty.tier","label":"Loyalty tier"');
    const items = placed.dataSources.directory.items as Array<{id: string; values: Record<string, unknown>}>;
    expect(items.find(item => item.id === 'maya-patel')?.values['example.loyalty.tier']).toBe('Gold');
    const unplaced = resolveContactsPeopleDirectory({query: defaultQuery, page: withValues, permissions: allPermissions, extensions: loyaltyDefaultSet});
    expect(JSON.stringify(unplaced.page.root)).not.toContain('example.loyalty.tier');
  });
});
