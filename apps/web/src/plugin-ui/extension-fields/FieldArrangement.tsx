import React, {useId, useState} from 'react';
import {Alert, Badge, Button, Select, Skeleton} from '../../components/ui';
import {addableFields, addField, arrangementFrom, layoutFromArrangement, moveField, removeField, type Arrangement, type HostField} from './arrangement';
import type {ExtensionField, FieldPlacement, FieldZone, FieldZoneKind} from './model';
import './extension-fields.css';

export type ArrangementSaveResult = {status: 'saved'} | {status: 'failed'; message: string};

type Props = {
  recordNoun: string;
  status: 'loading' | 'ready' | 'error';
  zones: readonly FieldZone[];
  /** Every active field that can be placed, whoever may read its values. */
  fields: readonly ExtensionField[];
  placements: readonly FieldPlacement[];
  /** The host's own fields in each zone, in order, keyed by zone id. */
  hostFields: Readonly<Record<string, readonly HostField[]>>;
  onSave(layout: {arranged: string[]; placements: FieldPlacement[]}): Promise<ArrangementSaveResult>;
  onClose(): void;
};

const kindTitles: Record<FieldZoneKind, string> = {display: 'Record page', form: 'Edit form', list: 'Lists'};
const kindOrder: readonly FieldZoneKind[] = ['display', 'form', 'list'];

/**
 * Lets an administrator place fields anywhere on a record's page, form and
 * lists: into any zone the host offers, before, between or after the host's
 * own fields, or nowhere.
 */
export function FieldArrangement({recordNoun, status, zones, fields, placements, hostFields, onSave, onClose}: Props) {
  const headingId = useId();
  const [arrangement, setArrangement] = useState<Arrangement>(() => arrangementFrom(zones, hostFields, placements));
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState('');
  const [notice, setNotice] = useState('');
  const [choices, setChoices] = useState<Record<string, string>>({});
  const byId = new Map(fields.map(field => [field.id, field]));

  if (status === 'loading') {
    return <section className="field-arrangement" aria-labelledby={headingId} aria-busy="true">
      <h2 className="field-arrangement__title" id={headingId}>Arrange fields</h2>
      <Skeleton label="Loading field arrangement" lines={3} />
    </section>;
  }
  if (status === 'error') {
    return <section className="field-arrangement" aria-labelledby={headingId}>
      <h2 className="field-arrangement__title" id={headingId}>Arrange fields</h2>
      <Alert variant="danger" title="The arrangement could not be loaded">Check your connection and try again.</Alert>
      <div className="field-arrangement__actions"><Button variant="secondary" onClick={onClose}>Close</Button></div>
    </section>;
  }

  const change = (next: Arrangement, message: string) => {
    setArrangement(next);
    setNotice(message);
  };
  const save = async () => {
    setSaving(true);
    setFailure('');
    try {
      const result = await onSave(layoutFromArrangement(arrangement, fields));
      if (result.status === 'saved') {
        setNotice('Arrangement saved.');
        onClose();
      } else {
        setFailure(result.message);
      }
    } finally {
      setSaving(false);
    }
  };

  return <section className="field-arrangement" aria-labelledby={headingId} data-field-arrangement="">
    <div className="field-arrangement__header">
      <h2 className="field-arrangement__title" id={headingId}>Arrange fields</h2>
      <p className="field-arrangement__description">Choose where each field appears on every {recordNoun}, in what order, and whether it appears at all. Changes apply to everyone in this workspace.</p>
    </div>
    <p className="sr-only" role="status">{notice}</p>
    {failure && <Alert variant="danger" title="Arrangement not saved">{failure}</Alert>}
    {fields.length === 0 && <Alert variant="info" title="No fields to arrange">Fields appear here when an enabled feature adds them to this record.</Alert>}
    {fields.length > 0 && kindOrder.map(kind => {
      const kindZones = zones.filter(zone => zone.kind === kind);
      if (kindZones.length === 0) return null;
      return <div className="field-arrangement__kind" key={kind}>
        <h3 className="field-arrangement__kind-title">{kindTitles[kind]}</h3>
        {kindZones.map(zone => {
          const items = arrangement[zone.id] ?? [];
          const addable = addableFields(arrangement, zone.id, fields);
          const selected = choices[zone.id] && addable.some(field => field.id === choices[zone.id]) ? choices[zone.id] ?? '' : addable[0]?.id ?? '';
          return <fieldset className="field-arrangement__zone" key={zone.id}>
            <legend>{zone.label}</legend>
            <ol className="field-arrangement__list">
              {items.length === 0 && <li className="field-arrangement__empty">No fields here yet.</li>}
              {items.map((item, index) => {
                if (item.kind === 'host') return <li className="field-arrangement__item field-arrangement__item--host" key={`host-${item.key}`}><span>{item.label}</span></li>;
                const field = byId.get(item.id);
                if (!field) return null;
                return <li className="field-arrangement__item" key={`field-${item.id}`}>
                  <span className="field-arrangement__label">{field.label} <Badge>{field.pluginName}</Badge></span>
                  <span className="field-arrangement__controls">
                    <Button size="small" variant="quiet" type="button" disabled={saving || index === 0} aria-label={`Move ${field.label} up in ${zone.label}`} onClick={() => change(moveField(arrangement, zone.id, item.id, -1), `${field.label} moved up.`)}>Up</Button>
                    <Button size="small" variant="quiet" type="button" disabled={saving || index === items.length - 1} aria-label={`Move ${field.label} down in ${zone.label}`} onClick={() => change(moveField(arrangement, zone.id, item.id, 1), `${field.label} moved down.`)}>Down</Button>
                    <Button size="small" variant="quiet" type="button" disabled={saving} aria-label={`Remove ${field.label} from ${zone.label}`} onClick={() => change(removeField(arrangement, zone.id, item.id), `${field.label} removed from ${zone.label}.`)}>Remove</Button>
                  </span>
                </li>;
              })}
            </ol>
            {addable.length > 0 && <div className="field-arrangement__add">
              <Select label={`Add a field to ${zone.label}`} value={selected} disabled={saving} onChange={event => setChoices(current => ({...current, [zone.id]: event.target.value}))}>
                {addable.map(field => <option key={field.id} value={field.id}>{field.label} ({field.pluginName})</option>)}
              </Select>
              <Button size="small" variant="secondary" type="button" disabled={saving || !selected} onClick={() => {
                const field = byId.get(selected);
                if (field) change(addField(arrangement, zone.id, field.id), `${field.label} added to ${zone.label}.`);
              }}>Add</Button>
            </div>}
          </fieldset>;
        })}
      </div>;
    })}
    <div className="field-arrangement__actions">
      <Button type="button" loading={saving} disabled={fields.length === 0} onClick={() => { void save(); }}>Save arrangement</Button>
      <Button type="button" variant="quiet" disabled={saving} onClick={onClose}>Cancel</Button>
    </div>
  </section>;
}
