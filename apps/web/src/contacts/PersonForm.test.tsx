import {renderToStaticMarkup} from 'react-dom/server';
import {describe, expect, it} from 'vitest';
import {ContactsApiError} from './api';
import {createFixtureContactsClient, fixturePersonFields, fixtureRecord} from './fixture-client';
import {people} from './people-data';
import {extensionChanges, mapPersonWriteError, personInput, personValues, PersonForm, validatePerson} from './PersonForm';
import {loyaltyDefaultSet, loyaltyPersonFields} from '../plugin-ui/extension-fields/fixtures';
import {contactsPersonPermissions, resolveContactsPersonForm} from './person-metadata';

const noop = () => undefined;
const allPermissions = new Set<string>(Object.values(contactsPersonPermissions));
const client = createFixtureContactsClient();
const customNames = fixturePersonFields.map(field => field.apiName);

describe('metadata-driven person form', () => {
  it('renders the plugin-owned definition with labelled standard and custom fields', () => {
    const metadata = resolveContactsPersonForm('create', allPermissions);
    expect(JSON.stringify(metadata)).not.toContain('contacts.people.');
    const markup = renderToStaticMarkup(<PersonForm client={client} onNavigate={noop} />);
    expect(markup).toContain('data-page-id="platform.contacts.person-form"');
    expect(markup).toContain('data-field-api="full_name"');
    expect(markup).toContain('for="person-full-name"');
    expect(markup).toContain('id="person-full-name-description"');
    expect(markup).toContain('data-field-api="service_tier__c"');
    expect(markup).toContain('Save person');
    expect(markup).toContain('Before you save');
    expect(markup).not.toContain('<main class="meta-page"');
  });

  it('uses the same definition for an existing person and preserves the deep link values', () => {
    const markup = renderToStaticMarkup(<PersonForm client={client} personId="maya-patel" onNavigate={noop} />);
    expect(markup).toContain('Edit Maya Patel');
    expect(markup).toContain('value="maya@northstar.example"');
    expect(markup).toContain('value="northstar" selected=""');
    expect(markup).toContain('value="priority" selected=""');
    expect(markup).toContain('Save changes');
    expect(markup).toContain('editing version 7');
  });

  it('filters tenant custom fields before rendering when the caller lacks permission', () => {
    const permissions = new Set([contactsPersonPermissions.read, contactsPersonPermissions.write]);
    const metadata = resolveContactsPersonForm('create', permissions);
    const encoded = JSON.stringify(metadata);
    expect(encoded).not.toContain('relationship_type__c');
    expect(encoded).not.toContain('service_tier__c');
    expect(encoded).not.toContain(contactsPersonPermissions.customFields);
    const markup = renderToStaticMarkup(<PersonForm client={client} onNavigate={noop} permissions={permissions} />);
    expect(markup).toContain('data-field-api="full_name"');
    expect(markup).not.toContain('data-field-api="relationship_type__c"');
    expect(markup).not.toContain('data-field-api="service_tier__c"');
  });

  it('does not expose the form when the plugin or write operation is unavailable', () => {
    const readOnly = new Set([contactsPersonPermissions.read]);
    expect(renderToStaticMarkup(<PersonForm client={client} onNavigate={noop} permissions={readOnly} />)).toContain('Person form unavailable');
    expect(renderToStaticMarkup(<PersonForm client={client} onNavigate={noop} pluginEnabled={false} />)).toContain('Person form unavailable');
  });

  it('maps authoritative validation, denial, duplicate, network, and concurrency results without changing values', async () => {
    const values = {full_name: '', email: 'invalid', telephone: '123'};
    expect(validatePerson(values)).toEqual({
      full_name: 'Enter the person’s full name.',
      email: 'Enter a valid email address.',
      telephone: 'Enter a phone number with at least 7 digits.',
    });
    const latest = async () => ({values: {full_name: 'Maya Patel', email: 'maya.patel@northstar.example'}, etag: '8'});
    await expect(mapPersonWriteError(new ContactsApiError(403, 'auth.permission_denied', 'denied'), 'create')).resolves.toMatchObject({status: 'failure', category: 'denied'});
    await expect(mapPersonWriteError(new ContactsApiError(400, 'validation.failed', 'invalid', [{field: 'email', code: 'person.duplicate', detail: 'exists'}]), 'create')).resolves.toMatchObject({status: 'failure', category: 'duplicate'});
    await expect(mapPersonWriteError(new ContactsApiError(400, 'validation.failed', 'invalid', [{field: 'telephone', code: 'telephone.invalid', detail: 'Enter at least 7 digits.'}]), 'create')).resolves.toEqual({status: 'validation', fieldErrors: {telephone: 'Enter at least 7 digits.'}, formError: undefined});
    await expect(mapPersonWriteError(new ContactsApiError(0, 'network.unreachable', 'offline'), 'create')).resolves.toMatchObject({status: 'failure', category: 'network'});
    await expect(mapPersonWriteError(new ContactsApiError(503, 'contacts.unavailable', 'down'), 'edit')).resolves.toMatchObject({status: 'failure', category: 'network'});
    await expect(mapPersonWriteError(new ContactsApiError(404, 'contacts.record_not_found', 'gone'), 'edit')).resolves.toMatchObject({status: 'failure', category: 'unavailable'});
    await expect(mapPersonWriteError(new ContactsApiError(409, 'contacts.version_conflict', 'changed'), 'edit', latest)).resolves.toEqual({status: 'conflict', latestEtag: '8', latestValues: {full_name: 'Maya Patel', email: 'maya.patel@northstar.example'}});
    await expect(mapPersonWriteError(new ContactsApiError(409, 'contacts.version_conflict', 'changed'), 'edit', async () => { throw new ContactsApiError(0, 'network.unreachable', 'offline'); })).resolves.toMatchObject({status: 'failure', category: 'network'});
  });

  it('saves through the client with only configured custom fields and reports the saved version', async () => {
    const conflicting = createFixtureContactsClient({write: 'conflict'});
    await expect(conflicting.updatePerson('maya-patel', 7, personInput({full_name: 'Maya Patel'}, customNames))).rejects.toMatchObject({status: 409});
    await expect(conflicting.getPerson('maya-patel')).resolves.toMatchObject({version: 8, email: 'maya.patel@northstar.example'});

    expect(personInput({full_name: ' Maya Patel ', email: '', telephone: ' +44 20 7946 0271 ', organisation_id: '', tags: 'operations, priority,,', relationship_type__c: 'Client', service_tier__c: '', job_title: 'Service Manager'}, customNames)).toEqual({
      full_name: 'Maya Patel',
      email: null,
      telephone: '+44 20 7946 0271',
      organisation_id: null,
      tags: ['operations', 'priority'],
      custom_fields: {relationship_type__c: 'Client'},
    });
    expect(personValues(fixtureRecord(people[4]), customNames)).toEqual({full_name: 'Maya Patel', email: 'maya@northstar.example', telephone: '+44 20 7946 0271', organisation_id: 'northstar', tags: 'operations, priority', relationship_type__c: 'Client', service_tier__c: 'priority'});
    expect(personValues(undefined, customNames)).toEqual({full_name: '', email: '', telephone: '', organisation_id: '', tags: ''});

    // Values the form did not show survive an update: an unconfigured custom
    // value and the organisation link of a caller who cannot read organisations.
    const existing = {...fixtureRecord(people[4]), customFields: {relationship_type__c: 'Client', legacy__c: 'kept', annual_value__c: 1200, consented__c: true, regions__c: ['North', 'Wales']}};
    expect(personInput({full_name: 'Maya Patel', relationship_type__c: ''}, ['relationship_type__c'], existing)).toMatchObject({organisation_id: 'northstar', custom_fields: {legacy__c: 'kept', annual_value__c: 1200, consented__c: true, regions__c: ['North', 'Wales']}});
    expect(personInput({full_name: 'Maya Patel', organisation_id: ''}, [], existing)).toMatchObject({organisation_id: null});
    // Configured custom fields the form did not submit keep their stored type and value.
    expect(personInput({full_name: 'Maya Patel'}, ['annual_value__c', 'consented__c', 'regions__c'], existing).custom_fields).toEqual(existing.customFields);
    expect(personValues(existing, ['relationship_type__c', 'annual_value__c', 'consented__c', 'regions__c'])).not.toHaveProperty('annual_value__c');
    expect(personValues(existing, ['relationship_type__c', 'annual_value__c'])).toMatchObject({relationship_type__c: 'Client'});

    const created = await client.createPerson(personInput({full_name: 'New Person', email: 'new@example.test'}, customNames));
    expect(created).toMatchObject({name: 'New Person', email: 'new@example.test', version: 1, archived: false});
    await expect(client.getPerson(created.id)).resolves.toMatchObject({id: created.id});
  });

  it('hides custom fields the workspace has not configured and offers only known organisations', () => {
    const metadata = resolveContactsPersonForm('create', allPermissions, {configuredCustomFields: new Set(['service_tier__c']), organisations: [{id: 'org-1', name: 'Only Org'}]});
    const encoded = JSON.stringify(metadata);
    expect(encoded).not.toContain('relationship_type__c');
    expect(encoded).toContain('service_tier__c');
    expect(encoded).toContain('Only Org');
    const none = resolveContactsPersonForm('create', allPermissions, {configuredCustomFields: new Set()});
    expect(JSON.stringify(none)).not.toContain('custom-fields-section');
    const markup = renderToStaticMarkup(<PersonForm client={createFixtureContactsClient({fields: []})} onNavigate={noop} />);
    expect(markup).not.toContain('data-field-api="service_tier__c"');
    expect(markup).not.toContain('Workspace fields');
  });

  it('hides the organisation field from callers who cannot read organisations', () => {
    const permissions = new Set([contactsPersonPermissions.read, contactsPersonPermissions.write]);
    const metadata = resolveContactsPersonForm('create', permissions, {permissions, organisations: [{id: 'org-1', name: 'Hidden Org'}]});
    const encoded = JSON.stringify(metadata);
    expect(encoded).not.toContain('organisation_id');
    expect(encoded).not.toContain('Hidden Org');
    const markup = renderToStaticMarkup(<PersonForm client={client} personId="maya-patel" onNavigate={noop} permissions={permissions} />);
    expect(markup).toContain('data-field-api="full_name"');
    expect(markup).not.toContain('data-field-api="organisation_id"');
  });

  it('renders loading, denied, and archived edit states without record values', () => {
    expect(renderToStaticMarkup(<PersonForm client={createFixtureContactsClient({record: 'loading'})} personId="maya-patel" onNavigate={noop} />)).toContain('Loading person details');
    const archived = renderToStaticMarkup(<PersonForm client={createFixtureContactsClient({record: 'archived'})} personId="maya-patel" onNavigate={noop} />);
    expect(archived).toContain('Archived person');
    expect(archived).not.toContain('value="maya@northstar.example"');
  });

  it('renders compact metadata without changing the contract or field set', () => {
    const wide = renderToStaticMarkup(<PersonForm client={client} formFactor="wide" onNavigate={noop} />);
    const compact = renderToStaticMarkup(<PersonForm client={client} formFactor="compact" onNavigate={noop} />);
    expect(wide).toContain('metadata-renderer--wide');
    expect(compact).toContain('metadata-renderer--compact');
    expect((wide.match(/data-field-api=/g) ?? []).length).toBe((compact.match(/data-field-api=/g) ?? []).length);
  });

  it('does not expose another record when the edit route is missing', () => {
    const markup = renderToStaticMarkup(<PersonForm client={createFixtureContactsClient({record: 'missing'})} personId="missing-person" onNavigate={noop} />);
    expect(markup).toContain('Person not found');
    expect(markup).not.toContain('maya@northstar.example');
  });

  it('resolves only the mode-appropriate resource operation', () => {
    const create = resolveContactsPersonForm('create', allPermissions);
    const edit = resolveContactsPersonForm('edit', allPermissions);
    expect(create?.page.actions.map(action => action.id)).toEqual(['create-person', 'cancel-person']);
    expect(edit?.page.actions.map(action => action.id)).toEqual(['update-person', 'cancel-person']);
  });
});

describe('fields other plugins add to the person form', () => {
  const extensionClient = () => createFixtureContactsClient({extensionFields: loyaltyDefaultSet, extensionValues: {'maya-patel': {'example.loyalty.tier': 'gold', 'example.loyalty.points': 1200}}});

  it('renders them as ordinary labelled controls in their placed zone', () => {
    const markup = renderToStaticMarkup(<PersonForm client={extensionClient()} personId="maya-patel" onNavigate={noop} />);
    expect(markup).toContain('Additional details');
    expect(markup).toContain('data-field-api="example.loyalty.tier"');
    expect(markup).toContain('value="gold" selected=""');
    expect(markup).toMatch(/data-field-api="example\.loyalty\.points"|type="number"[^>]*value="1200"/);
    expect(markup).toContain('type="date"');
    expect(markup).not.toContain('other features');
  });

  it('shows no empty section when nothing is placed there', () => {
    const markup = renderToStaticMarkup(<PersonForm client={client} personId="maya-patel" onNavigate={noop} />);
    expect(markup).not.toContain('Additional details');
  });

  it('sends only changed writable values, typed, with empty entries clearing', () => {
    const person = {...fixtureRecord(people[0]!), extensions: {'example.loyalty.tier': 'gold', 'example.loyalty.points': 1200}};
    const values = personValues(person, [], loyaltyPersonFields);
    expect(values['example.loyalty.points']).toBe('1200');
    expect(extensionChanges(values, loyaltyPersonFields, person)).toEqual({});
    const changed = {...values, 'example.loyalty.tier': '', 'example.loyalty.points': ' 1500 ', 'example.loyalty.member_since': '2020-02-03'};
    expect(extensionChanges(changed, loyaltyPersonFields, person)).toEqual({'example.loyalty.tier': null, 'example.loyalty.points': 1500, 'example.loyalty.member_since': '2020-02-03'});
    expect(personInput(changed, [], person, loyaltyPersonFields).extensions).toEqual({'example.loyalty.tier': null, 'example.loyalty.points': 1500, 'example.loyalty.member_since': '2020-02-03'});
    expect(personInput(values, [], person, loyaltyPersonFields)).not.toHaveProperty('extensions');
    const readOnly = loyaltyPersonFields.map(field => ({...field, writable: false}));
    expect(extensionChanges(changed, readOnly, person)).toEqual({});
  });

  it('validates numbers before saving', () => {
    expect(validatePerson({full_name: 'Maya', 'example.loyalty.points': 'many'}, loyaltyPersonFields)).toEqual({'example.loyalty.points': 'Enter a number for Points balance.'});
  });

  it('saves extension values with the person in one request', async () => {
    const local = extensionClient();
    const person = await local.getPerson('maya-patel');
    const saved = await local.updatePerson(person.id, person.version, {...personInput(personValues(person, [], loyaltyPersonFields), [], person, loyaltyPersonFields), extensions: {'example.loyalty.tier': 'silver'}});
    expect(saved.extensions).toEqual({'example.loyalty.tier': 'silver', 'example.loyalty.points': 1200});
    expect(saved.version).toBe(person.version + 1);
  });
});
