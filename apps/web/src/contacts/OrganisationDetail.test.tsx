import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import type {MetadataActionRequest} from '../plugin-ui/renderer/interactions';
import {MetadataPageRenderer} from '../plugin-ui/renderer/MetadataPageRenderer';
import {OrganisationDetail, executeOrganisationFixture} from './OrganisationDetail';
import {resolveContactsOrganisationDetail} from './organisation-metadata';
import {organisations} from './organisations-data';
import {people, peopleFixtureWorkspaceId} from './people-data';

const noop = () => undefined;
const permissions = new Set(['contacts.organisations.read', 'contacts.organisations.write', 'contacts.organisations.custom-fields.read', 'contacts.people.read', 'contacts.people.custom-fields.read']);
const northstar = organisations.find(item => item.id === 'northstar')!;

function render(options: Partial<Parameters<typeof OrganisationDetail>[0]> = {}) {
  return renderToStaticMarkup(<OrganisationDetail organisationId="northstar" workspaceId={peopleFixtureWorkspaceId} onNavigate={noop} {...options} />);
}

describe('metadata-driven organisation detail', () => {
  it('renders account values and navigable related people from declared data sources', () => {
    const markup = render();
    expect(markup).toContain('data-page-id="platform.contacts.organisation-detail"');
    expect(markup).toContain('Northstar Services');
    expect(markup).toContain('Business services');
    expect(markup).toContain('Maya Patel');
    expect(markup).toContain('maya@northstar.example');
    expect(markup).toContain('Edit organisation');
    expect(markup).toContain('Archive organisation');
  });

  it('renders the same detail metadata in compact form', () => {
    const markup = render({formFactor: 'compact'});
    expect(markup).toContain('metadata-renderer--compact');
    expect(markup).toContain('Maya Patel');
  });

  it('removes related people and their values when person read permission is absent', () => {
    const markup = render({callerPermissions: new Set(['contacts.organisations.read'])});
    expect(markup).not.toContain('People linked to this organisation');
    expect(markup).not.toContain('Maya Patel');
    expect(markup).not.toContain('maya@northstar.example');
  });

  it('shows an accessible empty relationship state', () => {
    const result = resolveContactsOrganisationDetail({organisation: northstar, state: 'ready', permissions, relatedPeople: []});
    expect(result.dataSources.related_people).toEqual({items: []});
    const markup = renderToStaticMarkup(<MetadataPageRenderer contextValues={{archived: false, has_people: false}} dataSources={result.dataSources} embedded host="internal-web" page={result.page} permissions={permissions} resources={result.resources} />);
    expect(markup).toContain('No people linked');
    expect(markup).toContain('Link a person');
  });

  it.each(['loading', 'missing', 'denied', 'error', 'disabled', 'workspace-changed'] as const)('does not leak organisation or related-person data in the %s state', fixtureState => {
    const markup = render({fixtureState, pluginEnabled: fixtureState !== 'disabled'});
    expect(markup).not.toContain('northstar.example');
    expect(markup).not.toContain('maya@northstar.example');
  });

  it('renders archived and version-changed states with correct actions', () => {
    const archived = render({fixtureState: 'archived'});
    expect(archived).toContain('Archived organisation');
    expect(archived).toContain('Restore organisation');
    expect(archived).not.toContain('Archive organisation');
    expect(render({fixtureState: 'version-changed'})).toContain('This organisation changed');
  });

  it('filters unconfigured organisation custom fields', () => {
    const markup = render({configuredFields: new Set(['name', 'sector', 'website', 'telephone', 'address', 'updated_at', 'version'])});
    expect(markup).not.toContain('Account status</dt>');
    expect(markup).not.toContain('Service region</dt>');
  });
});

describe('organisation action fixture', () => {
  const request = (id: string): MetadataActionRequest => ({action: {id, kind: 'resource-operation', target: `platform.contacts.organisation.${id}`}, values: {}, mode: 'view', recordId: 'northstar', etag: '7'});

  it('supports archive confirmation, restore, link, unlink, and failures', () => {
    expect(executeOrganisationFixture(request('archive-organisation'), 'ready', false, false, ['maya-patel'])).toMatchObject({archived: false, notice: 'Archive cancelled.'});
    expect(executeOrganisationFixture(request('archive-organisation'), 'ready', true, false, [])).toMatchObject({archived: true, result: {status: 'success', etag: '8'}});
    expect(executeOrganisationFixture(request('restore-organisation'), 'ready', true, true, [])).toMatchObject({archived: false, result: {status: 'success'}});
    const linked = executeOrganisationFixture(request('link-person'), 'ready', true, false, ['maya-patel']);
    expect(linked.relatedIds).toHaveLength(2);
    const unlinked = executeOrganisationFixture(request('unlink-person'), 'ready', true, false, linked.relatedIds);
    expect(unlinked.relatedIds).toHaveLength(1);
    expect(executeOrganisationFixture(request('link-person'), 'link-error', true, false, []).result).toMatchObject({status: 'failure', category: 'network'});
  });

  it('does not cross workspace-backed relationship fixtures', () => {
    const related = people.filter(person => person.organisation === northstar.name);
    expect(related.map(person => person.id)).toEqual(['maya-patel']);
  });
});
