import React, {useEffect, useState, type RefObject} from 'react';
import {Alert, Badge, Button, EmptyState, Select, Skeleton, TextField} from '../components/ui';
import {fieldTypes, isReference, ObjectsApiError, rollupFunctions, type RollupFunction, sharingModels, type FieldType, type SharingModel, type NewField, type ObjectAccess, type ObjectDescription, type ObjectField, type ObjectsClient, type ObjectSummary, type RolePermission, type ValidationIssue, type ValidationRule} from './api';
import {failed, objectRoute, useObject, type Load} from './ObjectPages';
import {Automations} from './Automations';
import {Issues, issuesText} from './issues';
import './objects.css';

type Navigate = (route: string) => void;

export const objectManagerRoute = '/settings/objects';
const managerObjectRoute = (apiName: string) => `${objectManagerRoute}/${encodeURIComponent(apiName)}`;

const origins: Record<ObjectSummary['origin'], string> = {system: 'Standard', plugin: 'Feature', workspace: 'Custom'};

/** Turns a label into a short API name: `Project stage` → `project_stage`. */
export function suggestName(label: string): string {
  const name = label.toLowerCase().normalize('NFKD').replace(/[^a-z0-9]+/g, '_').replace(/^[^a-z]+/, '').replace(/_+$/, '').slice(0, 63);
  return name.replace(/_+$/, '');
}

/** Setup → Object Manager: every object the caller can see, and new custom objects. */
export function ObjectManager({client, onNavigate, headingRef}: {client: ObjectsClient; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [load, setLoad] = useState<Load<{objects: ObjectSummary[]; canManage: boolean}>>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [creating, setCreating] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.listObjects(controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', value}); })
      .catch(error => { if (!controller.signal.aborted) setLoad(failed(error)); });
    return () => controller.abort();
  }, [client, attempt]);
  useEffect(() => { document.title = 'Object Manager - Limiara'; }, []);

  const canManage = load.status === 'ready' && load.value.canManage;
  return <div className="content-canvas object-page">
    <header className="page-header">
      <div><p className="page-header__breadcrumb">Settings</p><h1 ref={headingRef} tabIndex={-1}>Object Manager</h1><p>Objects hold your workspace's records. Custom objects and fields are yours to define.</p></div>
      {canManage && !creating && <div className="object-page__actions"><Button onClick={() => setCreating(true)}>New object</Button></div>}
    </header>
    {load.status === 'loading' && <Skeleton label="Loading objects" lines={4} />}
    {load.status === 'denied' && <Alert variant="warning" title="Permission needed">You do not have permission to manage objects.</Alert>}
    {(load.status === 'unavailable' || load.status === 'missing') && <div className="object-page__state"><Alert variant="danger" title="Objects could not be loaded">Try again in a moment.</Alert><Button variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>}
    {creating && <NewObjectForm client={client} onCancel={() => setCreating(false)} onCreated={object => onNavigate(managerObjectRoute(object.apiName))} />}
    {load.status === 'ready' && !canManage && <Alert variant="info" title="Read-only">You can see the objects you have access to. Ask an administrator with “Manage objects” to change them.</Alert>}
    {load.status === 'ready' && load.value.objects.length === 0 && !creating && <EmptyState title="No objects yet" description={canManage ? 'Create a custom object to start keeping records.' : 'Objects you can access will appear here.'} />}
    {load.status === 'ready' && load.value.objects.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Objects</caption>
      <thead><tr><th scope="col">Label</th><th scope="col">API name</th><th scope="col">Type</th><th scope="col">Key prefix</th><th scope="col">In navigation</th></tr></thead>
      <tbody>{load.value.objects.map(object => <tr key={object.apiName}>
        <th scope="row"><a href={managerObjectRoute(object.apiName)} onClick={event => { event.preventDefault(); onNavigate(managerObjectRoute(object.apiName)); }}>{object.label}</a></th>
        <td><code>{object.apiName}</code></td>
        <td><Badge variant={object.origin === 'workspace' ? 'selected' : 'neutral'}>{origins[object.origin]}</Badge></td>
        <td><code>{object.keyPrefix}</code></td>
        <td>{object.inNavigation ? 'Yes' : 'No'}</td>
      </tr>)}</tbody>
    </table></div>}
  </div>;
}

function NewObjectForm({client, onCancel, onCreated}: {client: ObjectsClient; onCancel: () => void; onCreated: (object: ObjectDescription) => void}) {
  const [label, setLabel] = useState('');
  const [plural, setPlural] = useState('');
  const [name, setName] = useState('');
  const [nameEdited, setNameEdited] = useState(false);
  const [inNavigation, setInNavigation] = useState(true);
  const [sharingModel, setSharingModel] = useState<SharingModel>('read_write');
  const [grantHierarchy, setGrantHierarchy] = useState(true);
  const [trackHistory, setTrackHistory] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<{message: string; issues: ValidationIssue[]} | null>(null);
  const shortName = nameEdited ? name : suggestName(label);
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setError(null);
    try {
      onCreated(await client.createObject({name: shortName, label: label.trim(), plural_label: plural.trim() || `${label.trim()}s`, in_navigation: inNavigation, sharing_model: sharingModel, grant_hierarchy: grantHierarchy, track_history: trackHistory}));
    } catch (reason) {
      setError(issuesText(reason, 'The object could not be created.'));
    } finally {
      setSaving(false);
    }
  };
  return <form className="object-form object-section" onSubmit={event => void submit(event)} aria-labelledby="new-object-heading">
    <h2 id="new-object-heading">New custom object</h2>
    {error && <Alert variant="danger" title={error.message}><Issues issues={error.issues} /></Alert>}
    <TextField label="Label" required value={label} maxLength={80} onChange={event => setLabel(event.target.value)} description="Singular, for example Project" />
    <TextField label="Plural label" required value={plural} maxLength={80} placeholder={label ? `${label}s` : ''} onChange={event => setPlural(event.target.value)} />
    <TextField label="API name" required value={shortName} maxLength={63} onChange={event => { setNameEdited(true); setName(event.target.value); }} description={`Saved as custom:${shortName || 'name'}. Lowercase letters, digits and single underscores; it cannot change later.`} />
    <label className="object-checkbox"><input type="checkbox" checked={inNavigation} onChange={event => setInNavigation(event.target.checked)} />Show in navigation</label>
    <RecordAccessFields sharingModel={sharingModel} grantHierarchy={grantHierarchy} onSharingModel={setSharingModel} onGrantHierarchy={setGrantHierarchy} />
    <label className="object-checkbox"><input type="checkbox" checked={trackHistory} onChange={event => setTrackHistory(event.target.checked)} />Track field history</label>
    <div className="object-page__actions"><Button type="submit" loading={saving} disabled={!label.trim() || !shortName}>Create object</Button><Button type="button" variant="secondary" onClick={onCancel} disabled={saving}>Cancel</Button></div>
  </form>;
}

const lookupFilterHelp = 'A condition on the related object, as written after WHERE in a query, such as custom:active = true. New values must meet it; values already set are kept.';

/** Sets or removes a lookup field's filter (ADR 0028). */
export function LookupFilterForm({client, object, field, onDone}: {client: ObjectsClient; object: ObjectDescription; field: ObjectField; onDone: (message?: string) => void}) {
  const [filter, setFilter] = useState(field.lookupFilter ?? '');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<{message: string; issues: ValidationIssue[]} | null>(null);
  const save = async (next: string | null) => {
    setSaving(true);
    setError(null);
    try {
      await client.updateField(object.apiName, field.apiName, field.version, {lookup_filter: next});
      onDone(next ? `${field.label} now offers only records that meet its filter.` : `${field.label} no longer has a filter.`);
    } catch (reason) {
      setError(issuesText(reason, 'The filter could not be saved.'));
    } finally {
      setSaving(false);
    }
  };
  return <form className="object-form" aria-label={`Lookup filter of ${field.label}`} onSubmit={event => { event.preventDefault(); void save(filter.trim() || null); }}>
    {error && <Alert variant="danger" title={error.message}><Issues issues={error.issues} /></Alert>}
    <div className="ui-field">
      <label className="ui-field__label" htmlFor="lookup-filter">Lookup filter for {field.label}</label>
      <span className="ui-field__description" id="lookup-filter-help">{lookupFilterHelp}</span>
      <textarea id="lookup-filter" className="ui-field__control object-textarea query-console__text" aria-describedby="lookup-filter-help" spellCheck={false} maxLength={1000} value={filter} onChange={event => setFilter(event.target.value)} />
    </div>
    <div className="object-page__actions">
      <Button type="submit" loading={saving} disabled={!filter.trim()}>Save filter</Button>
      {field.lookupFilter && <Button type="button" variant="secondary" onClick={() => void save(null)} disabled={saving}>Remove filter</Button>}
      <Button type="button" variant="secondary" onClick={() => onDone()} disabled={saving}>Cancel</Button>
    </div>
  </form>;
}

/** One object's fields and access in the Object Manager. */
export function ObjectManagerDetail({client, apiName, onNavigate, headingRef}: {client: ObjectsClient; apiName: string; onNavigate: Navigate; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [load, retry] = useObject(client, apiName);
  const [adding, setAdding] = useState(false);
  const [notice, setNotice] = useState('');
  const [canManage, setCanManage] = useState(false);
  const [filtering, setFiltering] = useState<string | null>(null);
  const object = load.status === 'ready' ? load.value : undefined;
  useEffect(() => {
    const controller = new AbortController();
    client.listObjects(controller.signal).then(value => { if (!controller.signal.aborted) setCanManage(value.canManage); }).catch(() => undefined);
    return () => controller.abort();
  }, [client]);
  useEffect(() => { if (object) document.title = `${object.label} - Object Manager - Limiara`; }, [object]);

  const toggleHistory = async (field: ObjectDescription['fields'][number], track: boolean) => {
    if (!object) return;
    try {
      await client.updateField(object.apiName, field.apiName, field.version, {track_history: track});
      setNotice(track ? `Changes to ${field.label} are now kept in history.` : `Changes to ${field.label} are no longer kept. Existing history stays.`);
      retry();
    } catch (reason) {
      setNotice(issuesText(reason, `History tracking for ${field.label} could not be changed.`).message);
    }
  };

  const removeField = async (field: ObjectDescription['fields'][number]) => {
    if (!object) return;
    try {
      await client.deleteField(object.apiName, field.apiName, field.version);
      setNotice(`${field.label} was deleted. Its values are no longer shown on records.`);
      retry();
    } catch (reason) {
      setNotice(issuesText(reason, `${field.label} could not be deleted.`).message);
    }
  };

  return <div className="content-canvas object-page">
    <header className="page-header">
      <div><p className="page-header__breadcrumb"><a href={objectManagerRoute} onClick={event => { event.preventDefault(); onNavigate(objectManagerRoute); }}>Object Manager</a></p><h1 ref={headingRef} tabIndex={-1}>{object?.label ?? 'Object'}</h1>{object && <p><code>{object.apiName}</code> · key prefix <code>{object.keyPrefix}</code> · {origins[object.origin]}</p>}</div>
      {object && <div className="object-page__actions"><Button variant="secondary" onClick={() => onNavigate(objectRoute(object.apiName))}>Open records</Button></div>}
    </header>
    {load.status === 'loading' && <Skeleton label="Loading object" lines={4} />}
    {load.status === 'missing' && <Alert variant="warning" title="Object not available">It may have been deleted, or you may not have access to it.</Alert>}
    {(load.status === 'denied' || load.status === 'unavailable') && <div className="object-page__state"><Alert variant="danger" title="The object could not be loaded">Try again in a moment.</Alert><Button variant="secondary" onClick={retry}>Try again</Button></div>}
    <div className="sr-only" aria-live="polite">{notice}</div>
    {notice && <Alert variant="info" title={notice} />}
    {object && <>
      <section className="object-section" aria-labelledby="fields-heading">
        <div className="object-section__header"><h2 id="fields-heading">Fields</h2>{canManage && !adding && <Button size="small" onClick={() => setAdding(true)}>New field</Button>}</div>
        {adding && <NewFieldForm client={client} object={object} onCancel={() => setAdding(false)} onCreated={field => { setAdding(false); setNotice(`${field} was added.`); retry(); }} />}
        <div className="object-table-wrap"><table className="object-table">
          <caption className="sr-only">Fields of {object.label}</caption>
          <thead><tr><th scope="col">Label</th><th scope="col">API name</th><th scope="col">Type</th><th scope="col">Attributes</th>{object.trackHistory && <th scope="col">History</th>}{canManage && <th scope="col"><span className="sr-only">Actions</span></th>}</tr></thead>
          <tbody>
            <tr><th scope="row">{object.nameLabel}</th><td><code>name</code></td><td>Text</td><td><Badge>Standard</Badge> <Badge>Required</Badge></td>{object.trackHistory && <td>Always</td>}{canManage && <td />}</tr>
            {object.fields.map(field => <tr key={field.apiName}>
              <th scope="row">{field.label}</th>
              <td><code>{field.apiName}</code></td>
              <td>{fieldTypes.find(type => type.value === field.type)?.label ?? field.type}{field.referenceObject ? ` (${field.referenceObject})` : ''}{field.type === 'formula' && field.formulaType ? ` (${fieldTypes.find(type => type.value === field.formulaType)?.label ?? field.formulaType})` : ''}{field.type === 'formula' && field.formula && <><br /><code>{field.formula}</code></>}{field.lookupFilter && <><br />Only records where <code>{field.lookupFilter}</code></>}</td>
              <td className="object-badges">{field.required && <Badge>Required</Badge>}{field.unique && <Badge>Unique</Badge>}{field.indexed && <Badge>Indexed</Badge>}{field.type === 'lookup' && <Badge>{field.deleteBehaviour === 'restrict' ? 'Restrict delete' : 'Clear on delete'}</Badge>}{field.type === 'master_detail' && <Badge>Deleted with its master</Badge>}{field.lookupFilter && <Badge>Filtered</Badge>}{field.type === 'rollup' && field.rollup && <Badge>{`${rollupFunctions.find(option => option.value === field.rollup?.function)?.label ?? field.rollup.function} of ${field.rollup.object}${field.rollup.field ? ` ${field.rollup.field}` : ''}`}</Badge>}</td>
              {object.trackHistory && <td>{canManage
                ? <input type="checkbox" aria-label={`Track history of ${field.label}`} checked={field.trackHistory} onChange={event => void toggleHistory(field, event.target.checked)} />
                : field.trackHistory ? 'Tracked' : 'Not tracked'}</td>}
              {canManage && <td>{isReference(field.type) && <Button size="small" variant="quiet" onClick={() => setFiltering(field.apiName)} aria-label={`Lookup filter of ${field.label}`}>Filter</Button>}{field.origin === 'workspace' && <Button size="small" variant="quiet" onClick={() => void removeField(field)} aria-label={`Delete ${field.label}`}>Delete</Button>}</td>}
            </tr>)}
          </tbody>
        </table></div>
        {filtering && object.fields.some(field => field.apiName === filtering) && <LookupFilterForm key={filtering} client={client} object={object} field={object.fields.find(field => field.apiName === filtering) ?? object.fields[0]!} onDone={message => { setFiltering(null); if (message) { setNotice(message); retry(); } }} />}
      </section>
      {canManage && <ValidationRules client={client} object={object} onNotice={setNotice} />}
      {canManage && <Automations client={client} object={object} onNotice={setNotice} />}
      {canManage && <RecordAccessSettings client={client} object={object} onSaved={message => { setNotice(message); retry(); }} />}
      {canManage && <ObjectPermissions client={client} object={object} />}
    </>}
  </div>;
}

type RuleLoad = {status: 'loading'} | {status: 'ready'; rules: ValidationRule[]} | {status: 'failed'};

/** Validation rules (ADR 0021): a rule blocks a save when its condition is true. */
export function ValidationRules({client, object, onNotice}: {client: Pick<ObjectsClient, 'validationRules' | 'createValidationRule' | 'updateValidationRule' | 'deleteValidationRule'>; object: ObjectDescription; onNotice: (message: string) => void}) {
  const [load, setLoad] = useState<RuleLoad>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [editing, setEditing] = useState<ValidationRule | 'new' | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.validationRules(object.apiName, controller.signal)
      .then(rules => { if (!controller.signal.aborted) setLoad({status: 'ready', rules}); })
      .catch(() => { if (!controller.signal.aborted) setLoad({status: 'failed'}); });
    return () => controller.abort();
  }, [client, object.apiName, attempt]);
  const fieldLabel = (apiName: string | null) => !apiName ? 'Record' : apiName === 'name' ? object.nameLabel : object.fields.find(field => field.apiName === apiName)?.label ?? apiName;
  const reload = (message: string) => { setEditing(null); onNotice(message); setAttempt(value => value + 1); };
  const toggle = async (rule: ValidationRule, active: boolean) => {
    try {
      await client.updateValidationRule(object.apiName, rule.name, rule.version, {active});
      reload(active ? `${rule.name} now checks every save.` : `${rule.name} is switched off.`);
    } catch (reason) {
      onNotice(issuesText(reason, `${rule.name} could not be changed.`).message);
    }
  };
  const remove = async (rule: ValidationRule) => {
    try {
      await client.deleteValidationRule(object.apiName, rule.name, rule.version);
      reload(`${rule.name} was deleted.`);
    } catch (reason) {
      onNotice(issuesText(reason, `${rule.name} could not be deleted.`).message);
    }
  };
  return <section className="object-section" aria-labelledby="rules-heading">
    <div className="object-section__header"><h2 id="rules-heading">Validation rules</h2>{editing === null && <Button size="small" onClick={() => setEditing('new')}>New rule</Button>}</div>
    <p>A rule blocks a save when its condition is true and shows its message. Rules run for every member, whatever records they can see.</p>
    {editing !== null && <RuleForm client={client} object={object} rule={editing === 'new' ? null : editing} onCancel={() => setEditing(null)} onSaved={reload} />}
    {load.status === 'loading' && <Skeleton label="Loading validation rules" lines={2} />}
    {load.status === 'failed' && <div className="object-page__state"><Alert variant="danger" title="Validation rules could not be loaded">Try again in a moment.</Alert><Button variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>}
    {load.status === 'ready' && load.rules.length === 0 && editing === null && <p className="object-value--empty">No validation rules yet.</p>}
    {load.status === 'ready' && load.rules.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Validation rules of {object.label}</caption>
      <thead><tr><th scope="col">Rule</th><th scope="col">Message</th><th scope="col">Shown on</th><th scope="col">Active</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>{load.rules.map(rule => <tr key={rule.name}>
        <th scope="row"><code>{rule.name}</code></th>
        <td>{rule.message}</td>
        <td>{fieldLabel(rule.errorField)}</td>
        <td><input type="checkbox" aria-label={`${rule.name} is active`} checked={rule.active} onChange={event => void toggle(rule, event.target.checked)} /></td>
        <td className="object-page__actions">
          <Button size="small" variant="quiet" aria-label={`Edit ${rule.name}`} onClick={() => setEditing(rule)}>Edit</Button>
          <Button size="small" variant="quiet" aria-label={`Delete ${rule.name}`} onClick={() => void remove(rule)}>Delete</Button>
        </td>
      </tr>)}</tbody>
    </table></div>}
  </section>;
}

function RuleForm({client, object, rule, onCancel, onSaved}: {client: Pick<ObjectsClient, 'createValidationRule' | 'updateValidationRule'>; object: ObjectDescription; rule: ValidationRule | null; onCancel: () => void; onSaved: (message: string) => void}) {
  const [name, setName] = useState(rule?.name ?? '');
  const [condition, setCondition] = useState(rule?.condition ?? '');
  const [message, setMessage] = useState(rule?.message ?? '');
  const [errorField, setErrorField] = useState(rule?.errorField ?? '');
  const [description, setDescription] = useState(rule?.description ?? '');
  const [active, setActive] = useState(rule?.active ?? true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<{message: string; issues: ValidationIssue[]} | null>(null);
  const conditionId = `${object.apiName}-rule-condition`;
  const issueFor = (field: string) => error?.issues.find(issue => issue.field === field)?.detail;
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const fields = {condition, message, error_field: errorField || null, description: description.trim() || null, active};
      if (rule) await client.updateValidationRule(object.apiName, rule.name, rule.version, fields);
      else await client.createValidationRule(object.apiName, {name: name.trim(), ...fields});
      onSaved(rule ? `${rule.name} was saved.` : `${name.trim()} was added and checks every save.`);
    } catch (reason) {
      setError(issuesText(reason, 'The rule could not be saved.'));
    } finally {
      setSaving(false);
    }
  };
  const conditionError = issueFor('condition');
  return <form className="object-form" onSubmit={event => void submit(event)} noValidate aria-busy={saving || undefined}>
    {error && <Alert variant="danger" title={error.message}><Issues issues={error.issues} /></Alert>}
    {rule ? <p className="object-form__meta">Rule <code>{rule.name}</code></p>
      : <TextField label="Rule name" description="Lowercase letters, digits and underscores, such as amount_positive." required value={name} error={issueFor('name')} maxLength={63} onChange={event => setName(event.target.value)} />}
    <div className="ui-field">
      <label className="ui-field__label" htmlFor={conditionId}>Error condition</label>
      <span className="ui-field__description" id={`${conditionId}-description`}>A formula that is true when the record is wrong, such as <code>{'custom:amount <= 0'}</code> or <code>{"ISPICKVAL(custom:stage, 'won') && ISBLANK(custom:close_date)"}</code>.</span>
      <textarea id={conditionId} className={`ui-field__control object-textarea query-console__text${conditionError ? ' ui-field__control--error' : ''}`} spellCheck={false} required value={condition} aria-invalid={conditionError ? true : undefined} aria-describedby={`${conditionId}-description${conditionError ? ` ${conditionId}-error` : ''}`} onChange={event => setCondition(event.target.value)} />
      {conditionError && <span className="ui-field__error" id={`${conditionId}-error`}>{conditionError}</span>}
    </div>
    <TextField label="Error message" required value={message} error={issueFor('message')} maxLength={255} onChange={event => setMessage(event.target.value)} />
    <Select label="Show the message on" value={errorField} error={issueFor('error_field')} onChange={event => setErrorField(event.target.value)}>
      <option value="">The record</option>
      <option value="name">{object.nameLabel}</option>
      {object.fields.map(field => <option key={field.apiName} value={field.apiName}>{field.label}</option>)}
    </Select>
    <TextField label="Description" description="Optional. Why the rule exists, for other administrators." value={description} error={issueFor('description')} maxLength={1000} onChange={event => setDescription(event.target.value)} />
    <label className="object-checkbox"><input type="checkbox" checked={active} onChange={event => setActive(event.target.checked)} /> Active</label>
    <div className="object-page__actions">
      <Button type="submit" loading={saving} disabled={saving}>{rule ? 'Save rule' : 'Add rule'}</Button>
      <Button type="button" variant="secondary" onClick={onCancel}>Cancel</Button>
    </div>
  </form>;
}

function NewFieldForm({client, object, onCancel, onCreated}: {client: ObjectsClient; object: ObjectDescription; onCancel: () => void; onCreated: (label: string) => void}) {
  const [label, setLabel] = useState('');
  const [type, setType] = useState<FieldType>('text');
  const [required, setRequired] = useState(false);
  const [unique, setUnique] = useState(false);
  const [indexed, setIndexed] = useState(false);
  const [trackHistory, setTrackHistory] = useState(false);
  const [options, setOptions] = useState('');
  const [reference, setReference] = useState('');
  const [behaviour, setBehaviour] = useState<'clear' | 'restrict'>('clear');
  const [lookupFilter, setLookupFilter] = useState('');
  const [targets, setTargets] = useState<ObjectSummary[]>([]);
  const [rollupObject, setRollupObject] = useState('');
  const [rollupDetail, setRollupDetail] = useState<ObjectDescription | null>(null);
  const [rollupRelationship, setRollupRelationship] = useState('');
  const [rollupFunction, setRollupFunction] = useState<RollupFunction>('count');
  const [rollupField, setRollupField] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<{message: string; issues: ValidationIssue[]} | null>(null);
  useEffect(() => {
    if (!isReference(type) && type !== 'rollup') return;
    const controller = new AbortController();
    client.listObjects(controller.signal).then(value => { if (!controller.signal.aborted) setTargets(value.objects); }).catch(() => undefined);
    return () => controller.abort();
  }, [client, type]);
  useEffect(() => {
    setRollupDetail(null);
    setRollupRelationship('');
    setRollupField('');
    if (type !== 'rollup' || !rollupObject) return;
    const controller = new AbortController();
    client.describe(rollupObject, controller.signal).then(detail => {
      if (controller.signal.aborted) return;
      setRollupDetail(detail);
      const first = detail.fields.find(field => field.type === 'master_detail' && field.referenceObject === object.apiName);
      setRollupRelationship(first?.apiName ?? '');
    }).catch(() => undefined);
    return () => controller.abort();
  }, [client, type, rollupObject, object.apiName]);
  const relationships = rollupDetail?.fields.filter(field => field.type === 'master_detail' && field.referenceObject === object.apiName) ?? [];
  const summarisable = rollupDetail?.fields.filter(field => ['number', 'currency', 'percent'].includes(field.type) || (rollupFunction !== 'sum' && ['date', 'date_time'].includes(field.type))) ?? [];
  const rollupReady = type !== 'rollup' || (rollupRelationship !== '' && (rollupFunction === 'count' || rollupField !== ''));
  const [formula, setFormula] = useState('');
  const [formulaType, setFormulaType] = useState<FieldType>('text');
  const picklist = type === 'picklist' || type === 'multi_picklist';
  const computed = type === 'formula';
  /** Formula and roll-up values are calculated, never entered. */
  const derived = computed || type === 'rollup';
  const mayBeUnique = ['text', 'email', 'phone', 'url', 'number'].includes(type);
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setError(null);
    const input: NewField = {name: suggestName(label), label: label.trim(), field_type: type, required, unique: mayBeUnique && unique, indexed, track_history: trackHistory};
    if (picklist) {
      input.options = options.split('\n').map(line => line.trim()).filter(Boolean).map((line, index) => ({value: suggestName(line) || `option_${index + 1}`, label: line}));
    }
    if (isReference(type)) input.reference_object = reference;
    if (isReference(type) && lookupFilter.trim()) input.lookup_filter = lookupFilter.trim();
    if (type === 'lookup') input.delete_behaviour = behaviour;
    if (type === 'master_detail') input.required = true;
    if (type === 'rollup') {
      input.rollup = {function: rollupFunction, object: rollupObject, relationship: rollupRelationship, ...(rollupFunction === 'count' ? {} : {field: rollupField})};
      input.required = false;
      input.indexed = false;
      input.track_history = false;
    }
    if (computed) {
      input.formula = formula;
      input.formula_type = formulaType;
      input.required = false;
      input.indexed = false;
      input.track_history = false;
    }
    try {
      await client.createField(object.apiName, input);
      onCreated(label.trim());
    } catch (reason) {
      setError(issuesText(reason, 'The field could not be created.'));
    } finally {
      setSaving(false);
    }
  };
  return <form className="object-form" onSubmit={event => void submit(event)} aria-label="New field">
    {error && <Alert variant="danger" title={error.message}><Issues issues={error.issues} /></Alert>}
    <Select label="Field type" value={type} onChange={event => setType(event.target.value as FieldType)}>
      {fieldTypes.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
    </Select>
    <TextField label="Label" required value={label} maxLength={80} onChange={event => setLabel(event.target.value)} description={label ? `API name custom:${suggestName(label)}` : undefined} />
    {picklist && <div className="ui-field">
      <label className="ui-field__label" htmlFor="new-field-options">Values</label>
      <span className="ui-field__description" id="new-field-options-help">One value per line</span>
      <textarea id="new-field-options" className="ui-field__control object-textarea" aria-describedby="new-field-options-help" value={options} onChange={event => setOptions(event.target.value)} required />
    </div>}
    {isReference(type) && <Select label={type === 'master_detail' ? 'Master object' : 'Related object'} required value={reference} onChange={event => setReference(event.target.value)}>
      <option value="">Choose an object</option>
      {targets.filter(target => target.apiName !== object.apiName || type === 'lookup').map(target => <option key={target.apiName} value={target.apiName}>{target.label}</option>)}
    </Select>}
    {isReference(type) && <div className="ui-field">
      <label className="ui-field__label" htmlFor="new-field-lookup-filter">Lookup filter (optional)</label>
      <span className="ui-field__description" id="new-field-lookup-filter-help">{lookupFilterHelp}</span>
      <textarea id="new-field-lookup-filter" className="ui-field__control object-textarea query-console__text" aria-describedby="new-field-lookup-filter-help" spellCheck={false} maxLength={1000} value={lookupFilter} onChange={event => setLookupFilter(event.target.value)} />
    </div>}
    {type === 'master_detail' && <p className="object-form__meta">Every record needs a master and stays with it. The master controls who can see and edit the record, and deleting or restoring the master deletes or restores the record with it. Only an object without records can get a master.</p>}
    {type === 'lookup' && <>
      <Select label="When the related record is deleted" value={behaviour} onChange={event => setBehaviour(event.target.value as 'clear' | 'restrict')}>
        <option value="clear">Clear this field</option>
        <option value="restrict">Don’t allow the deletion</option>
      </Select>
    </>}
    {type === 'rollup' && <>
      <Select label="Detail object" required value={rollupObject} onChange={event => setRollupObject(event.target.value)}>
        <option value="">Choose an object</option>
        {targets.filter(target => target.apiName !== object.apiName).map(target => <option key={target.apiName} value={target.apiName}>{target.label}</option>)}
      </Select>
      {rollupDetail && relationships.length === 0 && <p className="object-form__meta">{rollupDetail.label} has no master-detail field pointing to {object.label}.</p>}
      {relationships.length > 1 && <Select label="Through" value={rollupRelationship} onChange={event => setRollupRelationship(event.target.value)}>
        {relationships.map(field => <option key={field.apiName} value={field.apiName}>{field.label}</option>)}
      </Select>}
      <Select label="Summary" value={rollupFunction} onChange={event => { setRollupFunction(event.target.value as RollupFunction); setRollupField(''); }}>
        {rollupFunctions.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
      </Select>
      {rollupFunction !== 'count' && <Select label="Field to summarise" required value={rollupField} onChange={event => setRollupField(event.target.value)}>
        <option value="">Choose a field</option>
        {summarisable.map(field => <option key={field.apiName} value={field.apiName}>{field.label}</option>)}
      </Select>}
      <p className="object-form__meta">Kept up to date on every {object.label.toLowerCase()} as its detail records change.</p>
    </>}
    {computed && <>
      <div className="ui-field">
        <label className="ui-field__label" htmlFor="new-field-formula">Formula</label>
        <span className="ui-field__description" id="new-field-formula-help">Computed whenever a record is read, such as <code>{"custom:amount * 1.2"}</code> or <code>{"TEXT(custom:account.custom:region)"}</code>.</span>
        <textarea id="new-field-formula" className="ui-field__control object-textarea query-console__text" aria-describedby="new-field-formula-help" spellCheck={false} value={formula} onChange={event => setFormula(event.target.value)} required />
      </div>
      <Select label="Result type" value={formulaType} onChange={event => setFormulaType(event.target.value as FieldType)}>
        {fieldTypes.filter(option => ['text', 'number', 'currency', 'percent', 'boolean', 'date', 'date_time'].includes(option.value)).map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
      </Select>
    </>}
    {!derived && type !== 'master_detail' && <label className="object-checkbox"><input type="checkbox" checked={required} onChange={event => setRequired(event.target.checked)} />Required</label>}
    {mayBeUnique && <label className="object-checkbox"><input type="checkbox" checked={unique} onChange={event => setUnique(event.target.checked)} />Unique (ignores case)</label>}
    {!isReference(type) && !derived && <label className="object-checkbox"><input type="checkbox" checked={indexed} onChange={event => setIndexed(event.target.checked)} />Indexed for filters and sorting</label>}
    {object.trackHistory && !derived && <label className="object-checkbox"><input type="checkbox" checked={trackHistory} onChange={event => setTrackHistory(event.target.checked)} />Track history</label>}
    <div className="object-page__actions"><Button type="submit" loading={saving} disabled={!label.trim() || (isReference(type) && !reference) || (computed && !formula.trim()) || !rollupReady}>Add field</Button><Button type="button" variant="secondary" onClick={onCancel} disabled={saving}>Cancel</Button></div>
  </form>;
}

const accessColumns: {key: keyof ObjectAccess; label: string}[] = [
  {key: 'read', label: 'Read'},
  {key: 'create', label: 'Create'},
  {key: 'edit', label: 'Edit'},
  {key: 'delete', label: 'Delete'},
  {key: 'view_all', label: 'View all'},
  {key: 'modify_all', label: 'Modify all'},
];

function ObjectPermissions({client, object}: {client: ObjectsClient; object: ObjectDescription}) {
  const [load, setLoad] = useState<Load<RolePermission[]>>({status: 'loading'});
  const [notice, setNotice] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    client.permissions(object.apiName, controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', value}); })
      .catch(error => { if (!controller.signal.aborted) setLoad(failed(error)); });
    return () => controller.abort();
  }, [client, object.apiName]);
  const toggle = async (row: RolePermission, key: keyof ObjectAccess, checked: boolean) => {
    const access = {...row.access, [key]: checked};
    // Removing a weaker grant removes the stronger ones that imply it.
    if (!checked && key === 'read') Object.assign(access, {create: false, edit: false, delete: false, view_all: false, modify_all: false});
    if (!checked && key === 'edit') Object.assign(access, {delete: false, modify_all: false});
    if (!checked && key === 'delete') Object.assign(access, {modify_all: false});
    if (!checked && key === 'view_all') Object.assign(access, {modify_all: false});
    try {
      const saved = await client.setPermission(object.apiName, row.roleId, access);
      setLoad(current => current.status === 'ready' ? {status: 'ready', value: current.value.map(item => item.roleId === row.roleId ? {...item, access: saved} : item)} : current);
      setNotice(`${row.roleName} access to ${object.pluralLabel.toLowerCase()} saved.`);
    } catch {
      setNotice(`${row.roleName} access could not be saved.`);
    }
  };
  return <section className="object-section" aria-labelledby="access-heading">
    <h2 id="access-heading">Access by role</h2>
    <p>Stronger access includes weaker access: edit includes read, delete includes edit, and modify all includes everything. Members with the Manage objects, View all data or Modify all data permissions have that access to every object whatever their role shows here.</p>
    <div className="sr-only" aria-live="polite">{notice}</div>
    {load.status === 'loading' && <Skeleton label="Loading access" lines={2} />}
    {load.status !== 'loading' && load.status !== 'ready' && <Alert variant="danger" title="Access could not be loaded" />}
    {load.status === 'ready' && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Access to {object.pluralLabel} by role</caption>
      <thead><tr><th scope="col">Role</th>{accessColumns.map(column => <th scope="col" key={column.key}>{column.label}</th>)}</tr></thead>
      <tbody>{load.value.map(row => <tr key={row.roleId}>
        <th scope="row">{row.roleName}</th>
        {accessColumns.map(column => <td key={column.key}><input type="checkbox" aria-label={`${column.label} for ${row.roleName}`} checked={row.access[column.key]} onChange={event => void toggle(row, column.key, event.target.checked)} /></td>)}
      </tr>)}</tbody>
    </table></div>}
  </section>;
}

/** Who sees records by default, and whether managers see their reports' (ADR 0016). */
function RecordAccessFields({sharingModel, grantHierarchy, onSharingModel, onGrantHierarchy}: {sharingModel: SharingModel; grantHierarchy: boolean; onSharingModel: (value: SharingModel) => void; onGrantHierarchy: (value: boolean) => void}) {
  const description = sharingModels.find(model => model.value === sharingModel)?.description;
  return <>
    <Select label="Default record access" value={sharingModel} description={description} onChange={event => onSharingModel(event.target.value as SharingModel)}>
      {sharingModels.map(model => <option key={model.value} value={model.value}>{model.label}</option>)}
    </Select>
    <label className="object-checkbox"><input type="checkbox" checked={grantHierarchy} onChange={event => onGrantHierarchy(event.target.checked)} />Managers see and edit what their reports own</label>
  </>;
}

function RecordAccessSettings({client, object, onSaved}: {client: ObjectsClient; object: ObjectDescription; onSaved: (message: string) => void}) {
  const [sharingModel, setSharingModel] = useState<SharingModel>(object.sharingModel);
  const [grantHierarchy, setGrantHierarchy] = useState(object.grantHierarchy);
  const [trackHistory, setTrackHistory] = useState(object.trackHistory);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const changed = sharingModel !== object.sharingModel || grantHierarchy !== object.grantHierarchy || trackHistory !== object.trackHistory;
  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setError('');
    try {
      await client.updateObject(object.apiName, object.version, {sharing_model: sharingModel, grant_hierarchy: grantHierarchy, track_history: trackHistory});
      onSaved(`Record settings for ${object.pluralLabel.toLowerCase()} saved. They apply to every record immediately.`);
    } catch (reason) {
      setError(issuesText(reason, 'Record access could not be saved.').message);
    } finally {
      setSaving(false);
    }
  };
  return <form className="object-section object-form" onSubmit={event => void save(event)} aria-labelledby="record-access-heading">
    <h2 id="record-access-heading">Record settings</h2>
    <p>Owners always see and edit their records. Members with View all or Modify all see every record whatever this says.</p>
    {error && <Alert variant="danger" title={error} />}
    {object.fields.some(field => field.type === 'master_detail')
      ? <p className="object-form__meta">Controlled by the master: members see and edit these records when they can see and edit the master record.</p>
      : <RecordAccessFields sharingModel={sharingModel} grantHierarchy={grantHierarchy} onSharingModel={setSharingModel} onGrantHierarchy={setGrantHierarchy} />}
    <label className="object-checkbox"><input type="checkbox" checked={trackHistory} onChange={event => setTrackHistory(event.target.checked)} />Track field history: when records are created, deleted and restored, name and owner changes, and the fields marked below</label>
    <div className="object-page__actions"><Button type="submit" loading={saving} disabled={!changed}>Save record settings</Button></div>
  </form>;
}
