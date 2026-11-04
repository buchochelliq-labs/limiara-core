import React, {useEffect, useRef, useState} from 'react';
import {Alert, Badge, Button, Select, Skeleton, TextField} from '../components/ui';
import {automationTriggers, isReference, type Assignment, type Automation, type AutomationJob, type AutomationRun, type AutomationAction, type AutomationTiming, type AutomationTrigger, type ObjectDescription, type ObjectsClient, type ObjectSummary, type ValidationIssue} from './api';
import {Issues, issuesText} from './issues';

type Client = Pick<ObjectsClient, 'automations' | 'createAutomation' | 'updateAutomation' | 'deleteAutomation' | 'automationRuns' | 'automationJobs' | 'changeAutomationJob' | 'describe' | 'listObjects'>;
type AutomationLoad = {status: 'loading'} | {status: 'ready'; automations: Automation[]} | {status: 'failed'};

const timings: {value: AutomationTiming; label: string; description: string}[] = [
  {value: 'before_save', label: 'Before the record is saved', description: 'Set fields on the record itself.'},
  {value: 'after_save', label: 'After the record is saved', description: 'Create records, or update the records its lookups point to.'},
  {value: 'async', label: 'In the background', description: 'Shortly after the save, retried if it fails; the save does not wait.'},
  {value: 'scheduled', label: 'On a schedule', description: 'At a time set from a date on the record, or from the save, retried if it fails.'},
];

const timingWords: Record<AutomationTiming, string> = {before_save: 'before saving', after_save: 'after saving', async: 'in the background', scheduled: 'on a schedule'};
const background = (timing: AutomationTiming) => timing === 'async' || timing === 'scheduled';

const emptyAssignment = (): Assignment => ({field: '', formula: ''});

function startingActions(timing: AutomationTiming): AutomationAction[] {
  if (timing === 'after_save') return [{type: 'create_record', object: '', values: [{field: 'name', formula: ''}]}];
  return [{type: 'update_record', values: [emptyAssignment()]}];
}

/** When an automation runs, in words. */
export function automationWhen(automation: Pick<Automation, 'trigger' | 'timing'>): string {
  const trigger = automationTriggers.find(item => item.value === automation.trigger)?.label ?? automation.trigger;
  return `${trigger}, ${timingWords[automation.timing]}`;
}

/** What an automation does, in words. */
export function automationDoes(automation: Pick<Automation, 'actions'>, object: ObjectDescription, objects: ObjectSummary[]): string[] {
  const fieldLabel = (apiName: string) => apiName === 'name' ? object.nameLabel : object.fields.find(field => field.apiName === apiName)?.label ?? apiName;
  return automation.actions.map(action => {
    switch (action.type) {
      case 'update_record': return `Sets ${action.values.map(value => fieldLabel(value.field)).join(', ')}`;
      case 'update_related': return `Updates the ${fieldLabel(action.lookup)} record`;
      case 'create_record': return `Creates a ${objects.find(item => item.apiName === action.object)?.label ?? action.object} record`;
      case 'publish_event': return `Publishes the ${action.event} event`;
    }
  }).map((words, index) => automation.actions[index]?.when ? `${words} (when it applies)` : words);
}

/** Automations (ADR 0022): formulas that set fields or save other records when records are saved. */
export function Automations({client, object, onNotice}: {client: Client; object: ObjectDescription; onNotice: (message: string) => void}) {
  const [load, setLoad] = useState<AutomationLoad>({status: 'loading'});
  const [objects, setObjects] = useState<ObjectSummary[]>([]);
  const [attempt, setAttempt] = useState(0);
  const [editing, setEditing] = useState<Automation | 'new' | null>(null);
  const [showingRuns, setShowingRuns] = useState<string | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.automations(object.apiName, controller.signal)
      .then(automations => { if (!controller.signal.aborted) setLoad({status: 'ready', automations}); })
      .catch(() => { if (!controller.signal.aborted) setLoad({status: 'failed'}); });
    client.listObjects(controller.signal)
      .then(value => { if (!controller.signal.aborted) setObjects(value.objects); })
      .catch(() => undefined);
    return () => controller.abort();
  }, [client, object.apiName, attempt]);
  const reload = (message: string) => { setEditing(null); onNotice(message); setAttempt(value => value + 1); };
  const toggle = async (automation: Automation, active: boolean) => {
    try {
      await client.updateAutomation(object.apiName, automation.name, automation.version, {active});
      reload(active ? `${automation.name} now runs.` : `${automation.name} is switched off.`);
    } catch (reason) {
      onNotice(issuesText(reason, `${automation.name} could not be changed.`).message);
    }
  };
  const remove = async (automation: Automation) => {
    try {
      await client.deleteAutomation(object.apiName, automation.name, automation.version);
      reload(`${automation.name} was deleted.`);
    } catch (reason) {
      onNotice(issuesText(reason, `${automation.name} could not be deleted.`).message);
    }
  };
  return <section className="object-section" aria-labelledby="automations-heading">
    <div className="object-section__header"><h2 id="automations-heading">Automations</h2>{editing === null && <Button size="small" onClick={() => setEditing('new')}>New automation</Button>}</div>
    <p>An automation runs when records are saved. It can set fields before a record is saved, or create records and update related records afterwards. Automations run for every member, whatever they can see, and a failure stops the save.</p>
    {editing !== null && <AutomationForm client={client} object={object} objects={objects} automation={editing === 'new' ? null : editing} onCancel={() => setEditing(null)} onSaved={reload} />}
    {load.status === 'loading' && <Skeleton label="Loading automations" lines={2} />}
    {load.status === 'failed' && <div className="object-page__state"><Alert variant="danger" title="Automations could not be loaded">Try again in a moment.</Alert><Button variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>}
    {load.status === 'ready' && load.automations.length === 0 && editing === null && <p className="object-value--empty">No automations yet.</p>}
    {load.status === 'ready' && load.automations.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Automations of {object.label}</caption>
      <thead><tr><th scope="col">Automation</th><th scope="col">Runs when</th><th scope="col">Does</th><th scope="col">Active</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>{load.automations.map(automation => <tr key={automation.name}>
        <th scope="row"><code>{automation.name}</code>{automation.condition && <><br /><Badge>Conditional</Badge></>}</th>
        <td>{automationWhen(automation)}</td>
        <td>{automationDoes(automation, object, objects).map(line => <div key={line}>{line}</div>)}</td>
        <td><input type="checkbox" aria-label={`${automation.name} is active`} checked={automation.active} onChange={event => void toggle(automation, event.target.checked)} /></td>
        <td className="object-page__actions">
          <Button size="small" variant="quiet" aria-label={`Runs of ${automation.name}`} aria-expanded={showingRuns === automation.name} onClick={() => setShowingRuns(showingRuns === automation.name ? null : automation.name)}>Runs</Button>
          <Button size="small" variant="quiet" aria-label={`Edit ${automation.name}`} onClick={() => setEditing(automation)}>Edit</Button>
          <Button size="small" variant="quiet" aria-label={`Delete ${automation.name}`} onClick={() => void remove(automation)}>Delete</Button>
        </td>
      </tr>)}</tbody>
    </table></div>}
    {showingRuns && <AutomationRuns client={client} object={object} name={showingRuns} />}
    {showingRuns && load.status === 'ready' && background(load.automations.find(item => item.name === showingRuns)?.timing ?? 'before_save') && <AutomationJobs client={client} object={object} name={showingRuns} onNotice={onNotice} />}
  </section>;
}

const runEvents: Record<AutomationRun['event'], string> = {create: 'Created', update: 'Updated', delete: 'Deleted', undelete: 'Restored'};

type RunsLoad = {status: 'loading'} | {status: 'ready'; runs: AutomationRun[]; nextCursor: string | null} | {status: 'failed'};

/** The latest runs of one automation, with why failed runs failed. */
export function AutomationRuns({client, object, name}: {client: Pick<ObjectsClient, 'automationRuns'>; object: ObjectDescription; name: string}) {
  const [load, setLoad] = useState<RunsLoad>({status: 'loading'});
  const [loadingMore, setLoadingMore] = useState(false);
  // The automation being shown, so an older page that arrives after the
  // administrator switched automations is dropped.
  const showing = useRef(`${object.apiName}/${name}`);
  useEffect(() => {
    const controller = new AbortController();
    showing.current = `${object.apiName}/${name}`;
    setLoad({status: 'loading'});
    setLoadingMore(false);
    client.automationRuns(object.apiName, name, undefined, controller.signal)
      .then(page => { if (!controller.signal.aborted) setLoad({status: 'ready', ...page}); })
      .catch(() => { if (!controller.signal.aborted) setLoad({status: 'failed'}); });
    return () => controller.abort();
  }, [client, object.apiName, name]);
  const more = async () => {
    if (load.status !== 'ready' || !load.nextCursor) return;
    const requested = `${object.apiName}/${name}`;
    setLoadingMore(true);
    try {
      const page = await client.automationRuns(object.apiName, name, load.nextCursor);
      if (showing.current !== requested) return;
      setLoad({status: 'ready', runs: [...load.runs, ...page.runs], nextCursor: page.nextCursor});
    } catch {
      if (showing.current === requested) setLoad({status: 'failed'});
    } finally {
      if (showing.current === requested) setLoadingMore(false);
    }
  };
  return <div className="object-runs" aria-live="polite">
    <h3>Runs of <code>{name}</code></h3>
    <p>Each time its condition held. Runs are kept for 30 days; a failed run's change was not saved.</p>
    {load.status === 'loading' && <Skeleton label={`Loading runs of ${name}`} lines={2} />}
    {load.status === 'failed' && <Alert variant="danger" title="Runs could not be loaded">Try again in a moment.</Alert>}
    {load.status === 'ready' && load.runs.length === 0 && <p className="object-value--empty">No runs yet.</p>}
    {load.status === 'ready' && load.runs.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Runs of {name}</caption>
      <thead><tr><th scope="col">When</th><th scope="col">Record</th><th scope="col">On</th><th scope="col">Outcome</th><th scope="col">Records saved</th></tr></thead>
      <tbody>{load.runs.map(run => <tr key={run.id}>
        <td><time dateTime={run.ranAt}>{new Date(run.ranAt).toLocaleString()}</time>{run.ranBy && <><br />{run.ranBy.name ?? 'A member'}</>}</td>
        <td><code>{run.record}</code></td>
        <td>{runEvents[run.event]}</td>
        <td>{run.outcome === 'succeeded' ? <Badge>Succeeded</Badge> : <><Badge>Failed</Badge>{run.detail && <><br />{run.detail}</>}</>}</td>
        <td>{run.saves}</td>
      </tr>)}</tbody>
    </table></div>}
    {load.status === 'ready' && load.nextCursor && <Button size="small" variant="secondary" loading={loadingMore} disabled={loadingMore} onClick={() => void more()}>Show older runs</Button>}
  </div>;
}

const jobStatuses: Record<AutomationJob['status'], string> = {pending: 'Waiting', running: 'Running', succeeded: 'Done', failed: 'Failed', cancelled: 'Cancelled'};

type JobsLoad = {status: 'loading'} | {status: 'ready'; jobs: AutomationJob[]} | {status: 'failed'};

/** The queued jobs of a background or scheduled automation, with retry and cancel. */
export function AutomationJobs({client, object, name, onNotice}: {client: Pick<ObjectsClient, 'automationJobs' | 'changeAutomationJob'>; object: ObjectDescription; name: string; onNotice: (message: string) => void}) {
  const [load, setLoad] = useState<JobsLoad>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.automationJobs(object.apiName, name, controller.signal)
      .then(jobs => { if (!controller.signal.aborted) setLoad({status: 'ready', jobs}); })
      .catch(() => { if (!controller.signal.aborted) setLoad({status: 'failed'}); });
    return () => controller.abort();
  }, [client, object.apiName, name, attempt]);
  const change = async (job: AutomationJob, action: 'retry' | 'cancel') => {
    try {
      await client.changeAutomationJob(object.apiName, name, job.id, action);
      onNotice(action === 'retry' ? `The job for ${job.record} runs again shortly.` : `The job for ${job.record} was cancelled.`);
      setAttempt(value => value + 1);
    } catch (reason) {
      onNotice(issuesText(reason, 'The job could not be changed.').message);
    }
  };
  return <div className="object-runs" aria-live="polite">
    <h3>Queued jobs of <code>{name}</code></h3>
    <p>Each job waits until it is due, then runs once; failures are retried up to five times.</p>
    {load.status === 'loading' && <Skeleton label={`Loading jobs of ${name}`} lines={2} />}
    {load.status === 'failed' && <Alert variant="danger" title="Jobs could not be loaded">Try again in a moment.</Alert>}
    {load.status === 'ready' && load.jobs.length === 0 && <p className="object-value--empty">No jobs yet.</p>}
    {load.status === 'ready' && load.jobs.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Jobs of {name}</caption>
      <thead><tr><th scope="col">Record</th><th scope="col">Due</th><th scope="col">Status</th><th scope="col">Attempts</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>{load.jobs.map(job => <tr key={job.id}>
        <td><code>{job.record}</code></td>
        <td><time dateTime={job.dueAt}>{new Date(job.dueAt).toLocaleString()}</time></td>
        <td><Badge>{jobStatuses[job.status]}</Badge>{job.lastError && <><br />{job.lastError}</>}</td>
        <td>{job.attempts}</td>
        <td className="object-page__actions">
          {job.status === 'failed' && <Button size="small" variant="quiet" aria-label={`Retry the job for ${job.record}`} onClick={() => void change(job, 'retry')}>Retry</Button>}
          {job.status === 'pending' && <Button size="small" variant="quiet" aria-label={`Cancel the job for ${job.record}`} onClick={() => void change(job, 'cancel')}>Cancel</Button>}
        </td>
      </tr>)}</tbody>
    </table></div>}
  </div>;
}

/** The fields of the object an action writes, loaded once per object. */
function useTarget(client: Pick<ObjectsClient, 'describe'>, apiName: string, object: ObjectDescription): ObjectDescription | null {
  const [loaded, setLoaded] = useState<ObjectDescription | null>(null);
  useEffect(() => {
    if (!apiName || apiName === object.apiName) return;
    const controller = new AbortController();
    setLoaded(null);
    client.describe(apiName, controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoaded(value); })
      .catch(() => undefined);
    return () => controller.abort();
  }, [client, apiName, object.apiName]);
  if (!apiName) return null;
  return apiName === object.apiName ? object : loaded?.apiName === apiName ? loaded : null;
}

function ActionEditor({client, object, objects, timing, action, index, count, issueFor, onChange, onRemove}: {client: Pick<ObjectsClient, 'describe'>; object: ObjectDescription; objects: ObjectSummary[]; timing: AutomationTiming; action: AutomationAction; index: number; count: number; issueFor: (path: string) => string | undefined; onChange: (action: AutomationAction) => void; onRemove: () => void}) {
  const lookups = object.fields.filter(field => isReference(field.type));
  const event = action.type === 'publish_event';
  const targetApiName = action.type === 'update_record' || event ? object.apiName
    : action.type === 'update_related' ? lookups.find(field => field.apiName === action.lookup)?.referenceObject ?? ''
      : action.object;
  const target = useTarget(client, targetApiName, object);
  const path = `actions[${index}]`;
  const setValues = (values: Assignment[]) => onChange({...action, values});
  const setValue = (position: number, changes: Partial<Assignment>) => setValues(action.values.map((value, at) => at === position ? {...value, ...changes} : value));
  const setType = (type: string) => {
    if (type === 'update_related') onChange({type: 'update_related', lookup: '', values: [emptyAssignment()]});
    else if (type === 'update_record') onChange({type: 'update_record', values: [emptyAssignment()]});
    else if (type === 'publish_event') onChange({type: 'publish_event', event: '', values: [emptyAssignment()]});
    else onChange({type: 'create_record', object: '', values: [{field: 'name', formula: ''}]});
  };
  const legend = timing === 'before_save' ? 'Set fields on the record' : `Action ${index + 1}`;
  return <fieldset className="object-fieldset">
    <legend>{legend}</legend>
    {timing !== 'before_save' && <>
      <Select label="Do this" value={action.type} onChange={event => setType(event.target.value)}>
        {background(timing) && <option value="update_record">Update this record</option>}
        <option value="create_record">Create a record</option>
        <option value="update_related">Update a related record</option>
        <option value="publish_event">Publish an event</option>
      </Select>
      {action.type === 'publish_event' && <TextField label="Event name" description="Lowercase letters, digits and underscores, such as order_placed. Subscribers receive it with the fields below." className="object-formula" value={action.event} error={issueFor(`${path}.event`)} onChange={event => onChange({...action, event: event.target.value})} />}
      <TextField label={`Only when (action ${index + 1})`} description="Optional. A formula that must also be true for this action to run, so one automation can branch." className="object-formula" spellCheck={false} value={action.when ?? ''} error={issueFor(`${path}.when`)} onChange={event => onChange({...action, when: event.target.value || undefined})} />
      {action.type === 'create_record' && <Select label="Object" value={action.object} error={issueFor(`${path}.object`)} onChange={event => onChange({...action, object: event.target.value})}>
        <option value="">Choose an object</option>
        {objects.map(item => <option key={item.apiName} value={item.apiName}>{item.label}</option>)}
      </Select>}
      {action.type === 'update_related' && <Select label="The record in" description="A lookup on this record; nothing happens when it is empty." value={action.lookup} error={issueFor(`${path}.lookup`)} onChange={event => onChange({...action, lookup: event.target.value})}>
        <option value="">Choose a lookup</option>
        {lookups.map(field => <option key={field.apiName} value={field.apiName}>{field.label}</option>)}
      </Select>}
    </>}
    {issueFor(`${path}.values`) && <p className="ui-field__error">{issueFor(`${path}.values`)}</p>}
    {action.values.map((value, position) => {
      const at = `${path}.values[${position}]`;
      return <div className="object-assignment" key={position}>
        {event
          ? <TextField label={`Field ${position + 1}`} description="The event field's name, such as total." className="object-formula" value={value.field} error={issueFor(`${at}.field`)} onChange={event => setValue(position, {field: event.target.value})} />
          : <Select label={`Field ${position + 1}`} value={value.field} error={issueFor(`${at}.field`)} disabled={!target} onChange={event => setValue(position, {field: event.target.value})}>
            <option value="">{target ? 'Choose a field' : 'Choose the record first'}</option>
            {target && <option value="name">{target.nameLabel}</option>}
            {target?.fields.filter(field => field.type !== 'formula' && field.type !== 'rollup').map(field => <option key={field.apiName} value={field.apiName}>{field.label}</option>)}
          </Select>}
        <TextField label={`Formula for field ${position + 1}`} description="Evaluated on the saved record, such as custom:amount * 0.1 or 'Follow up ' & name." className="object-formula" spellCheck={false} value={value.formula} error={issueFor(`${at}.formula`)} onChange={event => setValue(position, {formula: event.target.value})} />
        {action.values.length > 1 && <Button type="button" size="small" variant="quiet" aria-label={`Remove field ${position + 1}`} onClick={() => setValues(action.values.filter((_, at) => at !== position))}>Remove</Button>}
      </div>;
    })}
    <div className="object-page__actions">
      <Button type="button" size="small" variant="secondary" onClick={() => setValues([...action.values, emptyAssignment()])}>Set another field</Button>
      {count > 1 && <Button type="button" size="small" variant="quiet" onClick={onRemove}>Remove action {index + 1}</Button>}
    </div>
  </fieldset>;
}

export function AutomationForm({client, object, objects, automation, onCancel, onSaved}: {client: Client; object: ObjectDescription; objects: ObjectSummary[]; automation: Automation | null; onCancel: () => void; onSaved: (message: string) => void}) {
  const [name, setName] = useState(automation?.name ?? '');
  const [trigger, setTrigger] = useState<AutomationTrigger>(automation?.trigger ?? 'create_or_update');
  const [timing, setTiming] = useState<AutomationTiming>(automation?.timing ?? 'before_save');
  const [condition, setCondition] = useState(automation?.condition ?? '');
  const [actions, setActions] = useState<AutomationAction[]>(automation ? structuredClone(automation.actions) : startingActions('before_save'));
  const [description, setDescription] = useState(automation?.description ?? '');
  const [offsetHours, setOffsetHours] = useState(String((automation?.schedule?.offset_minutes ?? 0) / 60));
  const [scheduleFrom, setScheduleFrom] = useState(automation?.schedule?.from ?? '');
  const dateFields = object.fields.filter(field => field.type === 'date' || field.type === 'date_time');
  const [active, setActive] = useState(automation?.active ?? true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<{message: string; issues: ValidationIssue[]} | null>(null);
  const conditionId = `${object.apiName}-automation-condition`;
  const issueFor = (field: string) => error?.issues.find(issue => issue.field === field)?.detail;
  const chooseTiming = (value: AutomationTiming) => { setTiming(value); setActions(startingActions(value)); };
  const lifecycle = trigger === 'delete' || trigger === 'undelete';
  const chooseTrigger = (value: AutomationTrigger) => {
    setTrigger(value);
    // Deleting and restoring can only act after saving.
    if (!automation && (value === 'delete' || value === 'undelete') && (timing === 'before_save' || timing === 'scheduled')) chooseTiming('after_save');
  };
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setError(null);
    const cleaned = actions.map(action => {
      const values = action.values.filter(value => value.field || value.formula.trim()).map(value => ({field: value.field.trim(), formula: value.formula.trim()}));
      const when = action.when?.trim();
      return {...action, values, when: when || undefined};
    });
    try {
      const schedule = timing === 'scheduled' ? {offset_minutes: Math.round(Number(offsetHours) * 60) || 0, from: scheduleFrom || null} : undefined;
      const fields = {description: description.trim(), trigger, condition: condition.trim(), actions: cleaned, active, ...(schedule ? {schedule} : {})};
      if (automation) await client.updateAutomation(object.apiName, automation.name, automation.version, fields);
      else await client.createAutomation(object.apiName, {name: name.trim(), timing, ...fields});
      onSaved(automation ? `${automation.name} was saved.` : `${name.trim()} was added${active ? ' and runs on the next save' : ''}.`);
    } catch (reason) {
      setError(issuesText(reason, 'The automation could not be saved.'));
    } finally {
      setSaving(false);
    }
  };
  const conditionError = issueFor('condition');
  return <form className="object-form" onSubmit={event => void submit(event)} noValidate aria-busy={saving || undefined}>
    {error && <Alert variant="danger" title={error.message}><Issues issues={error.issues} /></Alert>}
    {automation ? <p className="object-form__meta">Automation <code>{automation.name}</code>, {timingWords[timing]}</p>
      : <TextField label="Automation name" description="Lowercase letters, digits and underscores, such as set_region. Automations run in name order." required value={name} error={issueFor('name')} maxLength={63} onChange={event => setName(event.target.value)} />}
    <Select label="Runs when" value={trigger} error={issueFor('trigger')} onChange={event => chooseTrigger(event.target.value as AutomationTrigger)}>
      {automationTriggers.map(item => <option key={item.value} value={item.value}>{item.label}</option>)}
    </Select>
    {!automation && <fieldset className="object-fieldset">
      <legend>When it acts</legend>
      {timings.map(item => <label className="object-checkbox" key={item.value}><input type="radio" name={`${object.apiName}-automation-timing`} value={item.value} checked={timing === item.value} disabled={lifecycle && (item.value === 'before_save' || item.value === 'scheduled')} onChange={() => chooseTiming(item.value)} /> {item.label}: {item.description}</label>)}
    </fieldset>}
    {timing === 'scheduled' && <fieldset className="object-fieldset">
      <legend>When it runs</legend>
      <TextField label="Hours after" description="Negative runs that many hours before. Up to a year either way." type="number" step="0.25" value={offsetHours} error={issueFor('schedule.offset_minutes') ?? issueFor('schedule')} onChange={event => setOffsetHours(event.target.value)} />
      <Select label="Counting from" value={scheduleFrom} error={issueFor('schedule.from')} onChange={event => setScheduleFrom(event.target.value)}>
        <option value="">The save</option>
        {dateFields.map(field => <option key={field.apiName} value={field.apiName}>{field.label}</option>)}
      </Select>
    </fieldset>}
    <div className="ui-field">
      <label className="ui-field__label" htmlFor={conditionId}>Only when</label>
      <span className="ui-field__description" id={`${conditionId}-description`}>Optional. A formula that must be true for the automation to run, such as <code>{"ISPICKVAL(custom:stage, 'won')"}</code> or <code>{'ISCHANGED(custom:amount)'}</code>. Leave it empty to run on every save.</span>
      <textarea id={conditionId} className={`ui-field__control object-textarea query-console__text${conditionError ? ' ui-field__control--error' : ''}`} spellCheck={false} value={condition} aria-invalid={conditionError ? true : undefined} aria-describedby={`${conditionId}-description${conditionError ? ` ${conditionId}-error` : ''}`} onChange={event => setCondition(event.target.value)} />
      {conditionError && <span className="ui-field__error" id={`${conditionId}-error`}>{conditionError}</span>}
    </div>
    {issueFor('actions') && <p className="ui-field__error">{issueFor('actions')}</p>}
    {actions.map((action, index) => <ActionEditor key={index} client={client} object={object} objects={objects} timing={timing} action={action} index={index} count={actions.length} issueFor={issueFor}
      onChange={changed => setActions(actions.map((item, at) => at === index ? changed : item))}
      onRemove={() => setActions(actions.filter((_, at) => at !== index))} />)}
    {timing !== 'before_save' && <Button type="button" size="small" variant="secondary" onClick={() => setActions([...actions, ...startingActions('after_save')])}>Add an action</Button>}
    <TextField label="Description" description="Optional. Why the automation exists, for other administrators." value={description} error={issueFor('description')} maxLength={1000} onChange={event => setDescription(event.target.value)} />
    <label className="object-checkbox"><input type="checkbox" checked={active} onChange={event => setActive(event.target.checked)} /> Active</label>
    <div className="object-page__actions">
      <Button type="submit" loading={saving} disabled={saving}>{automation ? 'Save automation' : 'Add automation'}</Button>
      <Button type="button" variant="secondary" onClick={onCancel}>Cancel</Button>
    </div>
  </form>;
}
