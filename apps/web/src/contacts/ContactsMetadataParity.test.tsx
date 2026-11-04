import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {AppShell} from '../shell/AppShell';
import {shellFixture} from '../shell/data';
import {createFixtureContactsClient, fixturePeoplePage, fixtureRecord} from './fixture-client';
import {OrganisationDetail} from './OrganisationDetail';
import {resolveContactsOrganisationDetail, resolveContactsOrganisationDirectory, type OrganisationDirectoryQuery} from './organisation-metadata';
import {organisations} from './organisations-data';
import {people, peopleFixtureWorkspaceId} from './people-data';
import {resolveContactsPeopleDirectory, type PeopleDirectoryQuery} from './people-directory-metadata';
import {PersonDetail} from './PersonDetail';
import hostComponentSource from '../plugin-ui/renderer/hostComponents.tsx?raw';
import organisationDetailSource from './OrganisationDetail.tsx?raw';
import organisationDirectorySource from './OrganisationDirectory.tsx?raw';
import peopleDirectorySource from './PeopleDirectory.tsx?raw';
import personDetailSource from './PersonDetail.tsx?raw';

const noop = () => undefined;
const allPermissions = new Set(['contacts.people.read', 'contacts.people.write', 'contacts.people.custom-fields.read', 'contacts.organisations.read', 'contacts.organisations.write', 'contacts.organisations.custom-fields.read']);
const readPermissions = new Set(['contacts.people.read', 'contacts.organisations.read']);
const peopleQuery: PeopleDirectoryQuery = {query: '', label: 'All', sort: 'name', cursor: '', selected: 'maya-patel'};
const organisationQuery: OrganisationDirectoryQuery = {query: '', status: 'All', sort: 'name', cursor: '', selected: 'northstar'};
const peoplePage = fixturePeoplePage({search: '', label: 'All', sort: 'name', status: 'active', cursor: ''});
const maya = fixtureRecord(people[4]);
const client = createFixtureContactsClient();

describe('Contacts metadata lifecycle parity', () => {
  it('removes resources, operations, data-source declarations, actions, targets, and records when Contacts is disabled', () => {
    const peopleResult = resolveContactsPeopleDirectory({query: peopleQuery, state: 'disabled', pluginEnabled: false, page: peoplePage, selected: maya, permissions: allPermissions});
    const organisationResult = resolveContactsOrganisationDirectory({query: organisationQuery, state: 'disabled', pluginEnabled: false, permissions: allPermissions});
    for (const result of [peopleResult, organisationResult]) {
      expect(result.resources).toEqual([]);
      expect(result.page.resource).toBeNull();
      expect(result.page.data_sources).toEqual([]);
      expect(result.page.actions).toEqual([]);
      expect(JSON.stringify(result)).not.toContain('@northstar.example');
      expect(JSON.stringify(result.page.root)).not.toContain('contribution target');
    }
  });

  it('blocks every migrated direct route when the Contacts navigation contribution is disabled', () => {
    const model = {...shellFixture('ready'), navigation: []};
    for (const route of ['/contacts', '/contacts/people/maya-patel', '/contacts/people/maya-patel/edit', '/contacts/organisations', '/contacts/organisations/northstar']) {
      const markup = renderToStaticMarkup(<AppShell model={model} route={route} onNavigate={noop} onRetry={noop} onWorkspaceChange={noop} />);
      expect(markup).toContain('Route unavailable');
      expect(markup).not.toContain('maya@northstar.example');
      expect(markup).not.toContain('northstar.example');
      expect(markup).not.toContain('data-page-id="platform.contacts');
    }
  });

  it('re-enablement resolves fresh records and a different content hash', () => {
    const disabled = resolveContactsPeopleDirectory({query: peopleQuery, state: 'disabled', pluginEnabled: false, page: peoplePage, selected: maya, permissions: allPermissions});
    const enabled = resolveContactsPeopleDirectory({query: peopleQuery, page: peoplePage, selected: maya, permissions: allPermissions});
    expect(enabled.page.content_hash).not.toBe(disabled.page.content_hash);
    expect(JSON.stringify(enabled.dataSources)).toContain('maya@northstar.example');
    expect(enabled.page.selected_variant).toBe('selected');
  });

  it('permission reduction invalidates output and removes write/custom/relationship capabilities', () => {
    const fullPeople = resolveContactsPeopleDirectory({query: peopleQuery, page: peoplePage, selected: maya, permissions: allPermissions});
    const limitedPeople = resolveContactsPeopleDirectory({query: peopleQuery, page: peoplePage, selected: maya, permissions: new Set(['contacts.people.read'])});
    expect(fullPeople.page.content_hash).not.toBe(limitedPeople.page.content_hash);
    expect(JSON.stringify(limitedPeople.page)).not.toContain('add-person');
    expect(JSON.stringify(limitedPeople.page)).not.toContain('edit-person');
    expect(JSON.stringify(limitedPeople.page)).not.toContain('relationship_type__c');

    const fullOrganisation = resolveContactsOrganisationDetail({organisation: organisations[4], relatedPeople: [people[4]], state: 'ready', permissions: allPermissions});
    const limitedOrganisation = resolveContactsOrganisationDetail({organisation: organisations[4], relatedPeople: [people[4]], state: 'ready', permissions: readPermissions});
    expect(fullOrganisation.page.content_hash).not.toBe(limitedOrganisation.page.content_hash);
    expect(JSON.stringify(limitedOrganisation.page)).not.toContain('archive-organisation');
    expect(JSON.stringify(limitedOrganisation.page)).not.toContain('account_status__c');
  });

  it('workspace changes clear record and relationship values before rendering', () => {
    const otherWorkspace = '018f0000-0000-7000-8000-000000000002';
    const person = renderToStaticMarkup(<PersonDetail personId="maya-patel" workspaceId={otherWorkspace} client={client} onNavigate={noop} />);
    const organisation = renderToStaticMarkup(<OrganisationDetail organisationId="northstar" workspaceId={otherWorkspace} onNavigate={noop} />);
    expect(person).not.toContain('maya@northstar.example');
    expect(organisation).not.toContain('Maya Patel');
    expect(organisation).not.toContain('northstar.example');
    expect(person + organisation).toContain('different workspace');
  });
});

describe('Contacts renderer migration boundary', () => {
  it('keeps resource nouns and field names out of the generic host renderer', () => {
    expect(hostComponentSource).not.toMatch(/contacts|person|organisation|full_name|account_status__c/i);
  });

  it('keeps collection/detail entrypoints as metadata orchestration rather than feature layout trees', () => {
    for (const source of [peopleDirectorySource, personDetailSource, organisationDirectorySource, organisationDetailSource]) {
      expect(source).toContain('<MetadataPageRenderer');
      expect(source).not.toContain('<table');
      expect(source).not.toContain('<article');
      expect(source).not.toContain('<dl');
    }
  });

  it('uses the Northstar fixture workspace as the only record-bearing workspace', () => {
    expect(peopleFixtureWorkspaceId).toBe('018f0000-0000-7000-8000-000000000001');
  });
});
