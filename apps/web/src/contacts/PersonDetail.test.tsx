import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import cardContributionDocument from '../../../../fixtures/plugins/person-context/ui/extensions/person-card.json';
import actionContributionDocument from '../../../../fixtures/plugins/person-context/ui/extensions/person-action.json';
import relatedListContributionDocument from '../../../../fixtures/plugins/person-context/ui/extensions/person-related-list.json';
import timelineContributionDocument from '../../../../fixtures/plugins/person-context/ui/extensions/person-timeline.json';
import type {ExtensionContribution} from '../plugin-ui/contracts';
import {ContactsApiError} from './api';
import {createFixtureContactsClient, fixtureRecord} from './fixture-client';
import {people, peopleFixtureWorkspaceId} from './people-data';
import {arrangeFieldsPermission, describeRecordOperationError, PersonDetail, activityRefreshDelays, nextActivityRefresh, savePersonFieldLayout} from './PersonDetail';
import {loyaltyDefaultSet} from '../plugin-ui/extension-fields/fixtures';
import {resolveContactsPersonDetail} from './person-detail-metadata';

const noop = () => undefined;
const client = createFixtureContactsClient();
const allPermissions = new Set(['contacts.people.read', 'contacts.people.write', 'contacts.people.custom-fields.read', 'contacts.organisations.read']);
const contributions = [cardContributionDocument, actionContributionDocument, relatedListContributionDocument, timelineContributionDocument] as ExtensionContribution[];

function render(options: Partial<Parameters<typeof PersonDetail>[0]> = {}) {
  return renderToStaticMarkup(<PersonDetail personId="maya-patel" workspaceId={peopleFixtureWorkspaceId} client={client} onNavigate={noop} {...options} />);
}

describe('metadata-driven person detail', () => {
  it('renders the canonical person workspace from metadata', () => {
    const markup = render();
    expect(markup).toContain('data-page-id="platform.contacts.person-detail"');
    expect(markup).toContain('Maya Patel');
    expect(markup).toContain('maya@northstar.example');
    expect(markup).toContain('+44 20 7946 0271');
    expect(markup).toContain('Northstar Services');
    expect(markup).toContain('Edit person');
    expect(markup).toContain('Archive person');
  });

  it('renders the highlights panel, stage path, tabs, and activity timeline from metadata', () => {
    const markup = render();
    expect(markup).toContain('data-metadata-node="person-highlights"');
    expect(markup).toContain('aria-label="Person highlights"');
    expect(markup).toContain('role="tablist"');
    expect(markup).toContain('>Details</button>');
    expect(markup).toContain('>Related</button>');
    expect(markup).toContain('>Activity</button>');
    expect(markup).toContain('aria-current="step">Client</li>');
    expect(markup).toContain('data-complete="true">Prospect</li>');
    expect(markup).toContain('data-metadata-node="person-activity"');
    expect(markup).toContain('Person created');
    expect(markup).toContain('Version 7');
    expect(markup).toContain('<time dateTime="2026-09-16T09:00:00.000Z">');
  });

  it('shows the standard record fields in the record metadata section', () => {
    const markup = render();
    for (const label of ['Created', 'Created by', 'Last modified', 'Last modified by', 'Last touched', 'Version']) {
      expect(markup).toContain(`>${label}<`);
    }
    // Created by and Last modified by link to the members' user records.
    expect(markup).toContain('<a href="/users/usr-hannah">Hannah Okafor</a>');
    expect(markup).toContain('<a href="/users/usr-morgan">Morgan Lee</a>');
  });

  it('offers delete for a live person and undelete for one in the recycle bin', async () => {
    const live = render();
    expect(live).toContain('Delete person');
    expect(live).not.toContain('Undelete person');
    const bin = createFixtureContactsClient();
    const person = await bin.getPerson('maya-patel');
    const deleted = await bin.deletePerson?.(person.id, person.version);
    expect(deleted?.deleted).toBe(true);
    await expect(bin.deletePerson?.(person.id, deleted?.version ?? 0)).rejects.toMatchObject({status: 404});
    const restored = await bin.undeletePerson?.(person.id, deleted?.version ?? 0);
    expect(restored?.deleted).toBe(false);
  });

  it('keeps the stage path and activity out of reach without the custom-field or read permissions', () => {
    const standard = render({permissions: new Set(['contacts.people.read', 'contacts.people.write'])});
    expect(standard).not.toContain('data-metadata-node="person-stage"');
    expect(standard).toContain('data-metadata-node="person-activity"');
    const denied = render({permissions: new Set()});
    expect(denied).not.toContain('Person created');
    expect(denied).not.toContain('Maya Patel');
  });

  it('removes caller-inaccessible fields, relationships, and actions', () => {
    const markup = render({permissions: new Set(['contacts.people.read'])});
    expect(markup).not.toContain('Relationship</dt>');
    expect(markup).not.toContain('Service tier</dt>');
    expect(markup).not.toContain('Related organisation');
    expect(markup).not.toContain('Edit person');
    expect(markup).not.toContain('Archive person');
  });

  it('composes fixture cards, actions, related lists, and timeline items through stable slots', () => {
    const markup = render({permissions: allPermissions, contributions});
    expect(markup).toContain('Customer context');
    expect(markup).toContain('Quick edit');
    expect(markup).toContain('Organisation context');
    expect(markup).toContain('Recent activity');
    expect(markup).toContain('data-metadata-node="person-card-slot"');
    expect(markup).toContain('data-metadata-node="person-actions-slot"');
    expect(markup).toContain('data-metadata-node="person-related-list-slot"');
    expect(markup).toContain('data-metadata-node="person-timeline-slot"');
  });

  it('removes fixture contributions immediately when their plugin is disabled', () => {
    const enabled = render({permissions: allPermissions, contributions, enabledContributionPlugins: new Set(['example.person-context'])});
    const disabled = render({permissions: allPermissions, contributions, enabledContributionPlugins: new Set()});
    expect(enabled).toContain('Recent activity');
    expect(disabled).not.toContain('Customer context');
    expect(disabled).not.toContain('Quick edit');
    expect(disabled).not.toContain('Organisation context');
    expect(disabled).not.toContain('Recent activity');
    expect(disabled).not.toContain('person-card-slot');
  });

  it('changes the caller-safe content hash when enabled contributions change', () => {
    const enabled = render({permissions: allPermissions, contributions, enabledContributionPlugins: new Set(['example.person-context'])});
    const disabled = render({permissions: allPermissions, contributions, enabledContributionPlugins: new Set()});
    expect(enabled.match(/data-content-hash="([^"]+)/)?.[1]).not.toBe(disabled.match(/data-content-hash="([^"]+)/)?.[1]);
  });

  it('adds no landmarks or chrome for empty slots', () => {
    const markup = render();
    expect(markup).not.toContain('data-metadata-node="person-card-slot"');
    expect(markup).not.toContain('data-metadata-node="person-related-list-slot"');
    expect(markup).not.toContain('data-metadata-node="person-timeline-slot"');
  });

  it.each(['loading', 'missing', 'denied', 'error', 'disabled', 'workspace-changed'] as const)('does not expose record data in the %s state', forcedState => {
    const markup = render({forcedState, pluginEnabled: forcedState !== 'disabled'});
    expect(markup).not.toContain('maya@northstar.example');
    expect(markup).not.toContain('+44 20 7946 0271');
    expect(markup).not.toContain('Northstar Services');
  });

  it('clears record data when the active workspace changes', () => {
    const markup = render({workspaceId: '018f0000-0000-7000-8000-000000000002'});
    expect(markup).toContain('This person belongs to a different workspace');
    expect(markup).not.toContain('maya@northstar.example');
  });

  it('renders explicit archived and version-changed states', () => {
    const archived = render({client: createFixtureContactsClient({record: 'archived'})});
    expect(archived).toContain('Archived person');
    expect(archived).toContain('Restore person');
    expect(archived).not.toContain('Archive person');
    const changed = render({forcedState: 'version-changed'});
    expect(changed).toContain('This person changed');
  });

  it('renders client load failures as explicit states', () => {
    expect(render({client: createFixtureContactsClient({record: 'loading'})})).toContain('Loading person');
    expect(render({client: createFixtureContactsClient({record: 'missing'})})).toContain('Person not found');
    for (const markup of [render({client: createFixtureContactsClient({record: 'denied'})}), render({client: createFixtureContactsClient({record: 'error'})})]) {
      expect(markup).not.toContain('maya@northstar.example');
    }
  });

  it('offers restore only when the backing source can restore', () => {
    const {restorePerson: _, ...withoutRestore} = createFixtureContactsClient({record: 'archived'});
    const markup = render({client: withoutRestore});
    expect(markup).toContain('Archived person');
    expect(markup).not.toContain('Restore person');
    const resolved = resolveContactsPersonDetail({person: fixtureRecord(people[4], true), state: 'archived', permissions: allPermissions, canRestore: false});
    expect(resolved.page.actions.map(action => action.id)).not.toContain('restore-person');
  });
});

describe('person detail record operations', () => {
  it('archives and restores through the client with version checks', async () => {
    const local = createFixtureContactsClient();
    const archived = await local.archivePerson('maya-patel', 7);
    expect(archived).toMatchObject({archived: true, version: 8});
    await expect(local.archivePerson('maya-patel', 7)).rejects.toMatchObject({status: 409});
    await expect(local.restorePerson?.('maya-patel', 8)).resolves.toMatchObject({archived: false, version: 9});
  });

  it('maps archive and restore failures to explicit results', () => {
    expect(describeRecordOperationError(new ContactsApiError(409, 'contacts.version_conflict', 'changed'), 'archived')).toMatchObject({versionChanged: true, result: {status: 'failure', category: 'unavailable'}});
    expect(describeRecordOperationError(new ContactsApiError(404, 'contacts.record_not_found', 'gone'), 'archived')).toMatchObject({missing: true, result: {status: 'failure', category: 'unavailable'}});
    expect(describeRecordOperationError(new ContactsApiError(403, 'auth.permission_denied', 'no'), 'restored')).toMatchObject({result: {status: 'failure', category: 'denied'}});
    expect(describeRecordOperationError(new ContactsApiError(0, 'network.unreachable', 'offline'), 'restored').result).toMatchObject({status: 'failure', category: 'network', message: 'The person could not be restored. Try again.'});
  });
});

describe('timeline refresh after archive or restore', () => {
  const entry = (version: number) => ({id: `e${version}`, action: 'updated', version, actorId: 'usr-hannah', occurredAt: '2026-10-11T12:00:00Z'});

  it('stops once an entry for the written version appears', () => {
    expect(nextActivityRefresh({version: 8, attempt: 1}, [entry(7), entry(8)])).toBeNull();
  });

  it('keeps waiting when only older history arrives, even if the list grew', () => {
    const pending = {version: 8, attempt: 1};
    expect(nextActivityRefresh(pending, [entry(5), entry(6), entry(7)])).toBe(pending);
  });

  it('gives up after the last delay instead of retrying immediately', () => {
    expect(nextActivityRefresh({version: 8, attempt: activityRefreshDelays.length}, [entry(7)])).toBeNull();
    expect(nextActivityRefresh(null, [entry(8)])).toBeNull();
  });
});

describe('fields other plugins add to a person', () => {
  const values = {'maya-patel': {'example.loyalty.tier': 'gold', 'example.loyalty.points': 1200}};
  const extensionClient = (options: Parameters<typeof createFixtureContactsClient>[0] = {}) => createFixtureContactsClient({extensionFields: loyaltyDefaultSet, extensionValues: values, ...options});

  it('shows them inside the record like any other field, with no separate panel', () => {
    const markup = render({client: extensionClient()});
    expect(markup).not.toContain('other features');
    expect(markup).not.toContain('extension-fields');
    const contact = /data-metadata-node="person-identity-summary"[\s\S]*?<\/section>/.exec(markup)?.[0] ?? '';
    expect(contact).toContain('Loyalty tier');
    expect(contact).toContain('Gold');
    expect(contact).toContain('1,200');
  });

  it('follows the workspace arrangement, including positions among standard fields', () => {
    const arranged = {...loyaltyDefaultSet, placements: [{field_id: 'example.loyalty.tier', zone: 'highlights', position: 1}]};
    const markup = render({client: extensionClient({extensionFields: arranged})});
    const highlights = /data-metadata-node="person-highlights"[\s\S]*?<\/section>/.exec(markup)?.[0] ?? markup;
    expect(highlights.indexOf('Organisation')).toBeLessThan(highlights.indexOf('Loyalty tier'));
    expect(highlights.indexOf('Loyalty tier')).toBeLessThan(highlights.indexOf('Job title'));
    expect(markup).not.toContain('Points balance');
  });

  it('keeps the record usable when extension fields cannot load', () => {
    const markup = render({client: extensionClient({extensionFields: 'error'})});
    expect(markup).toContain('Maya Patel');
    expect(markup).not.toContain('Loyalty tier');
  });

  it('offers administrators an arrangement control and nobody else', () => {
    expect(render({client: extensionClient(), permissions: new Set([...allPermissions, arrangeFieldsPermission])})).toContain('Arrange fields');
    expect(render({client: extensionClient()})).not.toContain('Arrange fields');
    expect(render({client: createFixtureContactsClient(), permissions: new Set([...allPermissions, arrangeFieldsPermission])})).not.toContain('Arrange fields');
  });

  it('saves arrangements and explains conflicts and denial', async () => {
    const local = extensionClient();
    const layout = {arranged: ['example.loyalty.tier'], placements: [{field_id: 'example.loyalty.tier', zone: 'highlights', position: 0}]};
    expect(await savePersonFieldLayout(local, 0, layout)).toEqual({status: 'saved'});
    expect((await local.loadExtensionFields?.('platform.contacts.person'))?.placements).toEqual(layout.placements);
    expect(await savePersonFieldLayout(local, 0, layout)).toMatchObject({status: 'failed', message: expect.stringContaining('Someone else changed')});
    expect(await savePersonFieldLayout(extensionClient({write: 'denied'}), 0, layout)).toMatchObject({status: 'failed', message: expect.stringContaining('permission')});
    expect(await savePersonFieldLayout(createFixtureContactsClient(), 0, layout)).toMatchObject({status: 'failed'});
  });
});
