import React, {useEffect, useState} from 'react';
import {Alert, Badge, Button, Select, Skeleton, TextField} from '../components/ui';
import {formatValue, isReference, quickActionsMax, type ObjectDescription, type ObjectField, type ObjectsClient, type QuickAction, type QuickActionKind, type ValidationIssue} from './api';
import {editableFields, inputTypes, toApiValue, toDraftValue, type DraftValue} from './ObjectPages';
import {Issues, issuesText} from './issues';
import {suggestName} from './names';

type Client = Pick<ObjectsClient, 'quickActions' | 'createQuickAction' | 'updateQuickAction' | 'deleteQuickAction' | 'listObjects' | 'describe'>;
type ActionLoad = {status: 'loading'} | {status: 'ready'; actions: QuickAction[]} | {status: 'failed'};

/** A child object that can take a create action: one of its lookup or master-detail fields points at this object. */
export type ActionTarget = {object: ObjectDescription; field: ObjectField};

const targetKey = (object: string, field: string) => `${object}/${field}`;
/** How a target reads in a choice, such as “Contact (Account)”. */
export const targetLabel = (target: ActionTarget) => `${target.object.label} (${target.field.label})`;

/** The lookup and master-detail fields of other objects that point at `apiName`. */
export function actionTargets(objects: ObjectDescription[], apiName: string): ActionTarget[] {
  return objects.flatMap(object => object.fields.filter(field => isReference(field.type) && field.referenceObject === apiName).map(field => ({object, field})))
    .sort((left, right) => targetLabel(left).localeCompare(targetLabel(right)));
}

/** The fields an action's form may ask for or preset: the name and every entered field, never the reference it sets. */
export function actionFields(form: ObjectDescription, targetField: string | null): ObjectField[] {
  return editableFields(form).filter(field => field.apiName !== targetField);
}

/** Describes every object to find those that point at this one; failures simply offer fewer targets. */
function useTargets(client: Client, apiName: string): {status: 'loading' | 'ready'; targets: ActionTarget[]} {
  const [state, setState] = useState<{status: 'loading' | 'ready'; targets: ActionTarget[]}>({status: 'loading', targets: []});
  useEffect(() => {
    const controller = new AbortController();
    setState({status: 'loading', targets: []});
    client.listObjects(controller.signal)
      .then(({objects}) => Promise.all(objects.map(summary => client.describe(summary.apiName, controller.signal).catch(() => null))))
      .then(described => { if (!controller.signal.aborted) setState({status: 'ready', targets: actionTargets(described.filter((item): item is ObjectDescription => item !== null), apiName)}); })
      .catch(() => { if (!controller.signal.aborted) setState({status: 'ready', targets: []}); });
    return () => controller.abort();
  }, [client, apiName]);
  return state;
}

/** Quick actions (ADR 0032): record-page shortcuts that update chosen fields or create a related record. */
export function QuickActions({client, object, onNotice}: {client: Client; object: ObjectDescription; onNotice: (message: string) => void}) {
  const [load, setLoad] = useState<ActionLoad>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [editing, setEditing] = useState<QuickAction | 'new' | null>(null);
  const targets = useTargets(client, object.apiName);
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.quickActions(object.apiName, controller.signal)
      .then(actions => { if (!controller.signal.aborted) setLoad({status: 'ready', actions}); })
      .catch(() => { if (!controller.signal.aborted) setLoad({status: 'failed'}); });
    return () => controller.abort();
  }, [client, object.apiName, attempt]);
  const reload = (message: string) => { setEditing(null); onNotice(message); setAttempt(value => value + 1); };
  const remove = async (action: QuickAction) => {
    try {
      await client.deleteQuickAction(object.apiName, action.name, action.version);
      reload(`${action.label} was deleted.`);
    } catch (reason) {
      onNotice(issuesText(reason, `${action.label} could not be deleted.`).message);
    }
  };
  const targetOf = (action: QuickAction) => targets.targets.find(target => target.object.apiName === action.targetObject && target.field.apiName === action.targetField);
  const formOf = (action: QuickAction) => action.kind === 'update' ? object : targetOf(action)?.object;
  const fieldName = (form: ObjectDescription | undefined, apiName: string) => apiName === 'name' ? form?.nameLabel ?? 'Name' : form?.fields.find(field => field.apiName === apiName)?.label ?? apiName;
  const presetText = (form: ObjectDescription | undefined, apiName: string, value: unknown) => {
    const field = form?.fields.find(item => item.apiName === apiName);
    return `${fieldName(form, apiName)} = ${field ? formatValue(field, value) || 'blank' : String(value ?? 'blank')}`;
  };
  const full = load.status === 'ready' && load.actions.length >= quickActionsMax;
  return <section className="object-section" aria-labelledby="quick-actions-heading">
    <div className="object-section__header"><h2 id="quick-actions-heading">Quick actions</h2>{editing === null && load.status === 'ready' && !full && <Button size="small" onClick={() => setEditing('new')}>New quick action</Button>}</div>
    <p>Buttons on each {object.label.toLowerCase()} page, next to Edit and Delete. An action asks only for its fields and applies its presets. It grants nothing: members see it only where they could already edit the record or create the related one.</p>
    {full && <p className="object-form__meta">This object has the most quick actions it can have ({quickActionsMax}). Delete one to add another.</p>}
    {editing !== null && <QuickActionForm client={client} object={object} action={editing === 'new' ? null : editing} targets={targets} onCancel={() => setEditing(null)} onSaved={reload} />}
    {load.status === 'loading' && <Skeleton label="Loading quick actions" lines={2} />}
    {load.status === 'failed' && <div className="object-page__state"><Alert variant="danger" title="Quick actions could not be loaded">Try again in a moment.</Alert><Button variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>}
    {load.status === 'ready' && load.actions.length === 0 && editing === null && <p className="object-value--empty">No quick actions yet.</p>}
    {load.status === 'ready' && load.actions.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Quick actions of {object.label}</caption>
      <thead><tr><th scope="col">Action</th><th scope="col">Does</th><th scope="col">Position</th><th scope="col">Fields</th><th scope="col">Presets</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>{load.actions.map(action => {
        const form = formOf(action);
        const target = targetOf(action);
        return <tr key={action.name}>
          <th scope="row">{action.label}<br /><code>{action.name}</code></th>
          <td>{action.kind === 'update' ? 'Updates fields' : `Creates ${target ? targetLabel(target) : `${action.targetObject ?? ''} (${action.targetField ?? ''})`}`}</td>
          <td>{action.position}</td>
          <td>{action.fields.map(apiName => fieldName(form, apiName)).join(', ')}</td>
          <td className="object-badges">{Object.entries(action.presets).map(([apiName, value]) => <Badge key={apiName}>{presetText(form, apiName, value)}</Badge>)}</td>
          <td className="object-page__actions">
            <Button size="small" variant="quiet" aria-label={`Edit ${action.label}`} onClick={() => setEditing(action)}>Edit</Button>
            <Button size="small" variant="quiet" aria-label={`Delete ${action.label}`} onClick={() => void remove(action)}>Delete</Button>
          </td>
        </tr>;
      })}</tbody>
    </table></div>}
  </section>;
}

type PresetRow = {id: number; field: string; value: DraftValue};

/** A preset value as the API takes it; the record name is plain text. */
function presetValue(form: ObjectDescription, row: PresetRow): unknown {
  if (row.field === 'name') return typeof row.value === 'string' ? row.value.trim() : '';
  const field = form.fields.find(item => item.apiName === row.field);
  return field ? toApiValue(field, row.value) : row.value;
}

export function QuickActionForm({client, object, action, targets, onCancel, onSaved}: {client: Pick<ObjectsClient, 'createQuickAction' | 'updateQuickAction'>; object: ObjectDescription; action: QuickAction | null; targets: {status: 'loading' | 'ready'; targets: ActionTarget[]}; onCancel: () => void; onSaved: (message: string) => void}) {
  const [label, setLabel] = useState(action?.label ?? '');
  const [name, setName] = useState(action?.name ?? '');
  const [nameEdited, setNameEdited] = useState(false);
  const [kind, setKind] = useState<QuickActionKind>(action?.kind ?? 'update');
  const [position, setPosition] = useState(action ? String(action.position) : '');
  const [target, setTarget] = useState(action?.targetObject && action.targetField ? targetKey(action.targetObject, action.targetField) : '');
  const [fields, setFields] = useState<string[]>(action?.fields ?? []);
  const [presetRows, setPresetRows] = useState<PresetRow[] | null>(null);
  const [nextRow, setNextRow] = useState(0);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<{message: string; issues: ValidationIssue[]} | null>(null);
  const chosen = targets.targets.find(item => targetKey(item.object.apiName, item.field.apiName) === target);
  const form = kind === 'update' ? object : chosen?.object;
  const choices = form ? actionFields(form, kind === 'create' ? chosen?.field.apiName ?? null : null) : [];
  const fieldOf = (apiName: string) => choices.find(field => field.apiName === apiName);
  // Existing presets become rows once the form they belong to is known.
  const rows = presetRows ?? (form && action ? Object.entries(action.presets).map(([apiName, value], index) => ({id: -1 - index, field: apiName, value: apiName === 'name' ? String(value ?? '') : (() => { const field = fieldOf(apiName); return field ? toDraftValue(field, value) : String(value ?? ''); })()})) : []);
  const setRows = (change: (current: PresetRow[]) => PresetRow[]) => setPresetRows(change(rows));
  const shortName = action ? action.name : nameEdited ? name : suggestName(label);
  const issueFor = (field: string) => error?.issues.find(issue => issue.field === field)?.detail;
  const nameLabel = form?.nameLabel ?? 'Name';

  const switchForm = (apply: () => void) => {
    apply();
    // Fields and presets belong to the form's object, so a new kind or target starts them afresh.
    setFields([]);
    setPresetRows([]);
  };
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!form) return;
    setSaving(true);
    setError(null);
    const presets = Object.fromEntries(rows.filter(row => row.field).map(row => [row.field, presetValue(form, row)]));
    const ordered = ['name', ...choices.map(field => field.apiName)].filter(apiName => fields.includes(apiName));
    const parsed = position.trim() === '' ? undefined : Number(position);
    const at = parsed !== undefined && Number.isFinite(parsed) ? parsed : undefined;
    try {
      if (action) {
        await client.updateQuickAction(object.apiName, action.name, action.version, {label: label.trim(), fields: ordered, presets, ...(at === undefined ? {} : {position: at})});
        onSaved(`${label.trim()} was saved.`);
      } else {
        await client.createQuickAction(object.apiName, {name: shortName, label: label.trim(), kind, fields: ordered, presets, ...(at === undefined ? {} : {position: at}), ...(kind === 'create' && chosen ? {target_object: chosen.object.apiName, target_field: chosen.field.apiName} : {})});
        onSaved(`${label.trim()} was added to each ${object.label.toLowerCase()} page.`);
      }
    } catch (reason) {
      setError(issuesText(reason, 'The quick action could not be saved.'));
    } finally {
      setSaving(false);
    }
  };
  const fieldsError = issueFor('fields');
  const presetsError = issueFor('presets');
  const targetError = issueFor('target_object') ?? issueFor('target_field');
  return <form className="object-form" onSubmit={event => void submit(event)} noValidate aria-label={action ? `Edit ${action.label}` : 'New quick action'} aria-busy={saving || undefined}>
    {error && <Alert variant="danger" title={error.message}><Issues issues={error.issues} /></Alert>}
    <TextField label="Label" description="The button's text, such as Close or Add contact." required value={label} error={issueFor('label')} maxLength={80} onChange={event => setLabel(event.target.value)} />
    {action ? <p className="object-form__meta">Action <code>{action.name}</code></p>
      : <TextField label="Action name" description={`Lowercase letters, digits and underscores; it cannot change later.`} required value={shortName} error={issueFor('name')} maxLength={63} onChange={event => { setNameEdited(true); setName(event.target.value); }} />}
    {action ? <p className="object-form__meta">{action.kind === 'update' ? 'Updates fields of this record.' : `Creates ${chosen ? targetLabel(chosen) : `${action.targetObject ?? ''} (${action.targetField ?? ''})`} records, set to the current record.`} The kind and target cannot change.</p>
      : <Select label="Kind" value={kind} onChange={event => { const value = event.target.value as QuickActionKind; switchForm(() => setKind(value)); }}>
        <option value="update">Update fields of this record</option>
        <option value="create">Create a related record</option>
      </Select>}
    {!action && kind === 'create' && <Select label="Creates" required value={target} error={targetError} description={targets.status === 'loading' ? 'Loading the objects that point here…' : targets.targets.length === 0 ? `No object has a lookup or master-detail field pointing to ${object.label}.` : `The reference field is set to the current ${object.label.toLowerCase()}.`} onChange={event => { const value = event.target.value; switchForm(() => setTarget(value)); }}>
      <option value="">Choose an object</option>
      {targets.targets.map(item => <option key={targetKey(item.object.apiName, item.field.apiName)} value={targetKey(item.object.apiName, item.field.apiName)}>{targetLabel(item)}</option>)}
    </Select>}
    <TextField label="Position" type="number" min={0} step={1} description="Lower numbers come first. Leave empty to add it after the others." value={position} error={issueFor('position')} onChange={event => setPosition(event.target.value)} />
    {form && <fieldset className="object-multi">
      <legend>Fields on the form</legend>
      {[{apiName: 'name', label: nameLabel}, ...choices.map(field => ({apiName: field.apiName, label: field.label}))].map(item => {
        const itemError = issueFor(`fields.${item.apiName}`);
        return <label key={item.apiName} className="object-checkbox"><input type="checkbox" checked={fields.includes(item.apiName)} aria-invalid={itemError ? true : undefined} onChange={event => setFields(current => event.target.checked ? [...current, item.apiName] : current.filter(apiName => apiName !== item.apiName))} />{item.label}{itemError && <span className="ui-field__error">{itemError}</span>}</label>;
      })}
      {fieldsError && <span className="ui-field__error">{fieldsError}</span>}
    </fieldset>}
    {form && <fieldset className="object-multi">
      <legend>Presets</legend>
      <p className="object-form__meta">Values set on every save, whether or not the form shows the field. A shown field starts at its preset and the member may change it.</p>
      {rows.map((row, index) => {
        const number = index + 1;
        const field = fieldOf(row.field);
        const rowError = row.field ? issueFor(`presets.${row.field}`) : undefined;
        const used = new Set(rows.filter(other => other.id !== row.id).map(other => other.field));
        return <div key={row.id} className="object-form" role="group" aria-label={`Preset ${number}`}>
          <Select label={`Preset field ${number}`} value={row.field} onChange={event => { const apiName = event.target.value; const next = fieldOf(apiName); setRows(current => current.map(item => item.id === row.id ? {...item, field: apiName, value: next ? toDraftValue(next, next.options.find(option => option.default)?.value) : ''} : item)); }}>
            <option value="">Choose a field</option>
            {!used.has('name') && <option value="name">{nameLabel}</option>}
            {choices.filter(choice => !used.has(choice.apiName)).map(choice => <option key={choice.apiName} value={choice.apiName}>{choice.label}</option>)}
          </Select>
          {row.field && <PresetInput label={`Value for preset ${number}`} field={row.field === 'name' ? null : field ?? null} value={row.value} error={rowError} onChange={value => setRows(current => current.map(item => item.id === row.id ? {...item, value} : item))} />}
          <div className="object-page__actions"><Button type="button" size="small" variant="quiet" onClick={() => setRows(current => current.filter(item => item.id !== row.id))}>Remove preset {number}</Button></div>
        </div>;
      })}
      {presetsError && <span className="ui-field__error">{presetsError}</span>}
      <div className="object-page__actions"><Button type="button" size="small" variant="secondary" onClick={() => { setRows(current => [...current, {id: nextRow, field: '', value: ''}]); setNextRow(value => value + 1); }}>Add preset</Button></div>
    </fieldset>}
    {!form && kind === 'create' && !action && <p className="object-form__meta">Choose what the action creates to pick its fields.</p>}
    <div className="object-page__actions">
      <Button type="submit" loading={saving} disabled={saving || !form || !label.trim() || !shortName}>{action ? 'Save quick action' : 'Add quick action'}</Button>
      <Button type="button" variant="secondary" onClick={onCancel}>Cancel</Button>
    </div>
  </form>;
}

/** One preset's value, entered as the field's own input would take it. */
function PresetInput({label, field, value, error, onChange}: {label: string; field: ObjectField | null; value: DraftValue; error?: string; onChange: (value: DraftValue) => void}) {
  if (field?.type === 'boolean') return <label className="object-checkbox"><input type="checkbox" checked={value === true} aria-invalid={error ? true : undefined} onChange={event => onChange(event.target.checked)} />{label}{error && <span className="ui-field__error">{error}</span>}</label>;
  if (field?.type === 'picklist') return <Select label={label} value={String(value)} error={error} onChange={event => onChange(event.target.value)}>
    <option value="">None</option>
    {field.options.filter(option => option.active || option.value === value).map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
  </Select>;
  if (field?.type === 'multi_picklist') {
    const chosen = Array.isArray(value) ? value : [];
    return <fieldset className="object-multi"><legend>{label}</legend>
      {field.options.filter(option => option.active).map(option => <label key={option.value} className="object-checkbox"><input type="checkbox" checked={chosen.includes(option.value)} onChange={event => onChange(event.target.checked ? [...chosen, option.value] : chosen.filter(item => item !== option.value))} />{option.label}</label>)}
      {error && <span className="ui-field__error">{error}</span>}
    </fieldset>;
  }
  const reference = field && isReference(field.type);
  return <TextField label={label} type={field ? inputTypes[field.type] ?? 'text' : 'text'} step={field?.type === 'currency' ? '0.01' : field && ['number', 'percent'].includes(field.type) ? 'any' : undefined} description={reference ? `The ID of the ${field.referenceObject ?? 'related'} record.` : undefined} value={typeof value === 'string' ? value : ''} error={error} onChange={event => onChange(event.target.value)} />;
}
