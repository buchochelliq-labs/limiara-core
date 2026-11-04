import React, {useEffect, useMemo, useState} from 'react';
import {CheckCircle} from '@phosphor-icons/react';
import {Alert, Button} from '../components/ui';
import {useResponsiveFormFactor} from '../hooks/useResponsiveFormFactor';
import type {MetadataActionResult, MetadataInteractionAdapter, MetadataValues} from '../plugin-ui/renderer/interactions';
import {MetadataPageRenderer} from '../plugin-ui/renderer/MetadataPageRenderer';
import type {ExtensionField} from '../plugin-ui/extension-fields/model';
import {ContactsApiError, personResource, type ContactsClient, type PersonInput, type PersonRecord} from './api';
import {useExtensionFields} from './useExtensionFields';
import {contactsPersonPermissions, organisationsReadPermission, resolveContactsPersonForm} from './person-metadata';
import {useOrganisationOptions, usePersonFields} from './usePersonFields';
import './person-form.css';

export type PersonValues = {
  full_name: string;
  email: string;
  telephone: string;
  organisation_id: string;
  tags: string;
  [custom: string]: string | undefined;
};

export type PersonErrors = Record<string, string>;

const defaultPermissions = new Set<string>([...Object.values(contactsPersonPermissions), organisationsReadPermission]);

function text(value: unknown): string {
  return typeof value === 'string' ? value : typeof value === 'number' ? String(value) : '';
}

/**
 * Form values for an existing record: the standard fields plus the configured
 * custom fields the form can edit as text. Values of other types (numbers,
 * booleans, lists, objects) are not offered to the form; `personInput` keeps
 * them exactly as stored.
 */
export function personValues(person?: PersonRecord, customFields: Iterable<string> = [], extensionFields: readonly ExtensionField[] = []): PersonValues {
  const values: PersonValues = person
    ? {full_name: person.name, email: person.email, telephone: person.phone, organisation_id: person.organisationId, tags: person.tags.join(', ')}
    : {full_name: '', email: '', telephone: '', organisation_id: '', tags: ''};
  for (const name of customFields) {
    const value = person?.customFields[name];
    if (typeof value === 'string') values[name] = value;
  }
  for (const field of extensionFields) {
    const value = person?.extensions?.[field.id];
    values[field.id] = typeof value === 'boolean' ? String(value) : typeof value === 'number' || typeof value === 'string' ? String(value) : '';
  }
  return values;
}

/** The value a form entry stands for: empty clears, numbers and yes/no are typed. */
function extensionValue(field: ExtensionField, raw: unknown): unknown {
  const entry = text(raw).trim();
  if (!entry) return null;
  if (field.fieldType === 'number') return Number(entry);
  if (field.fieldType === 'boolean') return entry === 'true';
  return entry;
}

/**
 * Extension values the form changed, keyed by field id. Only writable fields
 * the form showed are considered; unchanged values are not sent.
 */
export function extensionChanges(values: MetadataValues, extensionFields: readonly ExtensionField[], existing?: PersonRecord): Record<string, unknown> {
  const changes: Record<string, unknown> = {};
  for (const field of extensionFields) {
    if (!field.writable || !Object.hasOwn(values, field.id)) continue;
    const next = extensionValue(field, values[field.id]);
    const stored = existing?.extensions?.[field.id] ?? null;
    if (next !== stored) changes[field.id] = next;
  }
  return changes;
}

/**
 * The API input for submitted form values. Only configured custom fields the
 * form actually submitted are written; every other custom value, and an
 * organisation link the form did not show, is preserved as stored, since the
 * API replaces the whole custom field map on update.
 */
export function personInput(values: MetadataValues, customFields: Iterable<string>, existing?: PersonRecord, extensionFields: readonly ExtensionField[] = []): PersonInput {
  const custom: Record<string, unknown> = {...existing?.customFields};
  for (const name of customFields) {
    if (!Object.hasOwn(values, name)) continue;
    const value = values[name];
    if (typeof value === 'string' ? value.trim() !== '' : value !== undefined && value !== null) custom[name] = value;
    else delete custom[name];
  }
  const organisationShown = Object.hasOwn(values, 'organisation_id');
  const extensions = extensionChanges(values, extensionFields, existing);
  return {
    ...(Object.keys(extensions).length ? {extensions} : {}),
    full_name: text(values.full_name).trim(),
    email: text(values.email).trim() || null,
    telephone: text(values.telephone).trim() || null,
    organisation_id: organisationShown ? text(values.organisation_id) || null : existing?.organisationId || null,
    tags: text(values.tags).split(',').map(tag => tag.trim()).filter(Boolean),
    custom_fields: custom,
  };
}

export function validatePerson(values: MetadataValues, extensionFields: readonly ExtensionField[] = []): PersonErrors {
  const errors: PersonErrors = {};
  for (const field of extensionFields) {
    const entry = text(values[field.id]).trim();
    if (!field.writable || !entry) continue;
    if (field.fieldType === 'number' && !Number.isFinite(Number(entry))) errors[field.id] = `Enter a number for ${field.label}.`;
    if (field.fieldType === 'text' && field.maxLength !== null && entry.length > field.maxLength) errors[field.id] = `${field.label} must be ${field.maxLength} characters or fewer.`;
  }
  const fullName = text(values.full_name);
  const email = text(values.email);
  const telephone = text(values.telephone);
  if (!fullName.trim()) errors.full_name = 'Enter the person’s full name.';
  if (email && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) errors.email = 'Enter a valid email address.';
  if (telephone && telephone.replace(/[^0-9]/g, '').length < 7) errors.telephone = 'Enter a phone number with at least 7 digits.';
  return errors;
}

/**
 * Maps a failed create or update to the renderer's result contract using the
 * API's stable problem codes. A version conflict resolves to the latest saved
 * values through `latest` so the person can review them.
 */
export async function mapPersonWriteError(error: unknown, mode: 'create' | 'edit', latest?: () => Promise<{values: MetadataValues; etag: string}>): Promise<MetadataActionResult> {
  const verb = mode === 'edit' ? 'edit' : 'add';
  if (!(error instanceof ContactsApiError)) return {status: 'failure', category: 'network', message: 'The person could not be saved. Check your connection and try again.'};
  if (error.status === 400 && error.code === 'validation.failed') {
    if (error.errors.some(issue => issue.code.includes('duplicate'))) {
      return {status: 'failure', category: 'duplicate', message: 'A person with a similar name and email may already exist. Review the details before saving again.'};
    }
    const fieldErrors = Object.fromEntries(error.errors.filter(issue => issue.field).map(issue => [issue.field, issue.detail || 'Check this value.']));
    return {status: 'validation', fieldErrors, formError: Object.keys(fieldErrors).length ? undefined : error.detail || 'Check the highlighted fields and try again.'};
  }
  if (error.status === 401 || error.status === 403) return {status: 'failure', category: 'denied', message: `You do not have permission to ${verb} people in this workspace.`};
  if (error.status === 404) return {status: 'failure', category: 'unavailable', message: 'This person may have been archived or removed. Return to people and try again.'};
  if (error.status === 409) {
    try {
      const current = await latest?.();
      if (current) return {status: 'conflict', latestValues: current.values, latestEtag: current.etag};
    } catch {
      // The latest version could not be read; fall through to a retryable failure.
    }
    return {status: 'failure', category: 'network', message: 'This person changed since you opened it, and the latest version could not be loaded. Try again.'};
  }
  return {status: 'failure', category: 'network', message: 'The person could not be saved. Check your connection and try again.'};
}

type RecordLoad =
  | {status: 'loading'}
  | {status: 'ready'; record: PersonRecord}
  | {status: 'missing'}
  | {status: 'denied'}
  | {status: 'error'};

function loadFailure(error: unknown): RecordLoad {
  const status = error instanceof ContactsApiError ? error.status : 0;
  return status === 404 ? {status: 'missing'} : status === 403 || status === 401 ? {status: 'denied'} : {status: 'error'};
}

function seededRecord(client: ContactsClient, id: string): RecordLoad {
  const seeded = client.seeds?.person?.(id);
  if (!seeded) return {status: 'loading'};
  return 'value' in seeded ? {status: 'ready', record: seeded.value} : loadFailure(seeded.error);
}

type PersonFormProps = {
  onNavigate: (route: string) => void;
  client: ContactsClient;
  personId?: string;
  permissions?: ReadonlySet<string>;
  pluginEnabled?: boolean;
  formFactor?: 'wide' | 'compact';
};

function unavailable(onNavigate: (route: string) => void, title: string, description: string) {
  return <div className="content-canvas person-form-page person-form-missing"><Alert variant="warning" title={title}>{description}</Alert><Button variant="secondary" onClick={() => onNavigate('/contacts')}>Return to people</Button></div>;
}

export function PersonForm({onNavigate, client, personId, permissions = defaultPermissions, pluginEnabled = true, formFactor}: PersonFormProps) {
  const responsiveFormFactor = useResponsiveFormFactor();
  const effectiveFormFactor = formFactor ?? responsiveFormFactor;
  const mode = personId ? 'edit' : 'create';
  const [load, setLoad] = useState<RecordLoad>(() => personId ? seededRecord(client, personId) : {status: 'loading'});
  const fields = usePersonFields(client, permissions);
  const organisations = useOrganisationOptions(client, permissions);
  const customNames = useMemo(() => new Set(fields?.map(field => field.apiName) ?? []), [fields]);
  const [extensionLoad] = useExtensionFields(client, personResource, pluginEnabled && permissions.has(contactsPersonPermissions.read));
  const extensionFields = extensionLoad.set.fields;

  useEffect(() => {
    if (!personId || !pluginEnabled) return;
    const controller = new AbortController();
    client.getPerson(personId, controller.signal)
      .then(record => { if (!controller.signal.aborted) setLoad({status: 'ready', record}); })
      .catch(error => { if (!controller.signal.aborted) setLoad(loadFailure(error)); });
    return () => controller.abort();
  }, [client, personId, pluginEnabled]);

  const person = load.status === 'ready' ? load.record : undefined;
  const initialValues = useMemo(() => personValues(person, customNames, extensionFields), [customNames, extensionFields, person]);
  const metadata = useMemo(() => pluginEnabled ? resolveContactsPersonForm(mode, permissions, {organisations: organisations ?? [], permissions, configuredCustomFields: customNames, extensions: extensionLoad.set}) : null, [customNames, extensionLoad.set, mode, organisations, permissions, pluginEnabled]);
  const [saved, setSaved] = useState<PersonRecord | null>(null);
  const returnRoute = person ? `/contacts/people/${encodeURIComponent(person.id)}` : '/contacts';
  const initialEtag = person ? `${person.version}` : undefined;
  const adapter = useMemo<MetadataInteractionAdapter>(() => ({
    execute: async request => {
      const fieldErrors = validatePerson(request.values, extensionFields);
      if (Object.keys(fieldErrors).length) return {status: 'validation', fieldErrors};
      const input = personInput(request.values, customNames, person, extensionFields);
      try {
        const record = personId
          ? await client.updatePerson(personId, Number(request.etag ?? person?.version ?? 0), input)
          : await client.createPerson(input);
        setSaved(record);
        return {status: 'success', values: request.values, etag: String(record.version)};
      } catch (error) {
        return mapPersonWriteError(error, mode, personId ? async () => {
          const latest = await client.getPerson(personId);
          return {values: personValues(latest, customNames, extensionFields), etag: String(latest.version)};
        } : undefined);
      }
    },
    navigate: () => onNavigate(returnRoute),
  }), [client, customNames, extensionFields, mode, onNavigate, person, personId, returnRoute]);

  if (mode === 'edit' && load.status === 'missing') return unavailable(onNavigate, 'Person not found', 'This person may have been archived, removed, or belong to another workspace.');
  if (mode === 'edit' && load.status === 'denied') return unavailable(onNavigate, 'Person unavailable', 'You do not have permission to edit this person.');
  if (mode === 'edit' && load.status === 'error') return unavailable(onNavigate, 'Person could not be loaded', 'Check your connection and try again.');
  if (!metadata) return unavailable(onNavigate, 'Person form unavailable', 'The Contacts plugin is disabled or you do not have permission to use this form.');
  if (fields === null || organisations === null) return unavailable(onNavigate, 'Person form could not be loaded', 'The workspace fields or organisations could not be read. Check your connection and try again.');
  // The renderer owns the typed values, so it mounts only once the record and
  // the catalogues it depends on are known; a later catalogue load must not
  // reset what the person has typed.
  if (fields === undefined || organisations === undefined || extensionLoad.status === 'loading' || (mode === 'edit' && (load.status === 'loading' || !person))) return <div className="content-canvas person-form-page person-form-missing" role="status" aria-live="polite"><p>Loading person details…</p></div>;
  if (person?.archived) return unavailable(onNavigate, 'Archived person', 'This record is read-only until it is restored.');

  if (saved) {
    return <div className="content-canvas person-form-page person-form-success" role="status">
      <CheckCircle aria-hidden="true" weight="fill" />
      <p className="app-eyebrow">{mode === 'edit' ? 'Changes saved' : 'Person added'}</p>
      <h1>{saved.name}</h1>
      <p>{mode === 'edit' ? 'The latest details are saved for this workspace.' : 'The new person is ready in this workspace.'}</p>
      <div className="person-form-success__actions">
        <Button onClick={() => onNavigate(`/contacts/people/${encodeURIComponent(saved.id)}`)}>{mode === 'edit' ? 'Return to person' : 'Open person'}</Button>
        <Button variant="secondary" onClick={() => onNavigate('/contacts')}>Return to people</Button>
      </div>
    </div>;
  }

  const messages = {
    'person.page.title': mode === 'edit' ? `Edit ${person?.name}` : 'Add person',
    'person.page.description': mode === 'edit' ? 'Update the details your team uses while preserving safe concurrency.' : 'Start with the details your team needs to recognise and contact this person.',
    'person.form.label': mode === 'edit' ? `Edit ${person?.name}` : 'Add person',
    'person.guidance.version': mode === 'edit' ? `You are editing version ${person?.version}. Conflicts will not silently overwrite newer work.` : 'You can add more fields after the record is created.',
  };

  return <div className="content-canvas person-form-page person-metadata-page">
    <MetadataPageRenderer
      contextValues={{mode}}
      embedded
      embeddedHeadingOffset={0}
      etag={initialEtag}
      focusOnRender
      formFactor={effectiveFormFactor}
      host="internal-web"
      initialValues={initialValues}
      interactionAdapter={adapter}
      messages={messages}
      mode={mode}
      page={metadata.page}
      permissions={permissions}
      recordId={personId}
      resources={[metadata.resource]}
    />
  </div>;
}
