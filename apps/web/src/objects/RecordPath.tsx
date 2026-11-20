import {useEffect, useRef} from 'react';
import {Button} from '../components/ui';
import {optionOutcome, type ObjectDescription, type ObjectField, type ObjectRecord, type PicklistOption} from './api';

/** The object's path field, when it is one the member can read (ADR 0037 §2). */
export function pathFieldOf(object: ObjectDescription): ObjectField | undefined {
  if (!object.pathField) return undefined;
  const field = object.fields.find(item => item.apiName === object.pathField);
  return field && field.type === 'picklist' && field.access?.read !== false ? field : undefined;
}

/** The path's steps: active open options in order, then the closed ones. */
export function pathSteps(field: ObjectField, current: string | null): {open: PicklistOption[]; closed: PicklistOption[]} {
  // An inactive option stays visible while the record holds it.
  const shown = field.options.filter(option => option.active || option.value === current);
  return {
    open: shown.filter(option => optionOutcome(option) === 'open'),
    closed: shown.filter(option => optionOutcome(option) !== 'open'),
  };
}

type RecordPathProps = {
  object: ObjectDescription;
  field: ObjectField;
  record: ObjectRecord;
  /** Whether the member may change the field on this record. */
  canEdit: boolean;
  busy: boolean;
  /** Saves an open value straight away. */
  onMove: (value: string) => void;
  /** Opens the record form with a closing value preset, so rules that need more fields to close are met in one save. */
  onClose: (option: PicklistOption) => void;
};

/**
 * The path on a record page (ADR 0037 §2): the path field's open values as
 * steps, the current one marked, and its closing values after them. On a
 * phone it is a strip that scrolls to the current step.
 */
export function RecordPath({object, field, record, canEdit, busy, onMove, onClose}: RecordPathProps) {
  const raw = record.values[field.apiName];
  const current = typeof raw === 'string' && raw ? raw : null;
  const {open, closed} = pathSteps(field, current);
  const index = open.findIndex(option => option.value === current);
  const closedCurrent = closed.find(option => option.value === current);
  const next = closedCurrent ? undefined : open[index + 1];
  const editable = canEdit && field.access?.edit !== false;
  const currentRef = useRef<HTMLLIElement>(null);

  useEffect(() => {
    // Keeps the current step in view on a narrow screen; nothing moves when it already is.
    currentRef.current?.scrollIntoView?.({block: 'nearest', inline: 'center'});
  }, [current]);

  const step = (option: PicklistOption, position: number) => {
    const isCurrent = option.value === current;
    const complete = !closedCurrent && index > position;
    const label = option.label;
    return <li key={option.value} ref={isCurrent ? currentRef : undefined} aria-current={isCurrent ? 'step' : undefined} data-complete={complete || undefined}>
      {editable && !isCurrent
        ? <button type="button" className="record-path__step" disabled={busy} onClick={() => onMove(option.value)}>{label}<span className="sr-only">: move to this {field.label.toLowerCase()}</span></button>
        : <span className="record-path__step">{label}{isCurrent && <span className="sr-only"> (current)</span>}</span>}
    </li>;
  };

  return <nav aria-label={`${object.label} ${field.label.toLowerCase()} path`} className="record-path">
    <ol className="record-path__steps">
      {open.map(step)}
      {closed.map(option => {
        const isCurrent = option.value === current;
        return <li key={option.value} ref={isCurrent ? currentRef : undefined} aria-current={isCurrent ? 'step' : undefined} data-outcome={optionOutcome(option)} className="record-path__closed">
          {editable && !isCurrent
            ? <button type="button" className="record-path__step" aria-haspopup="dialog" disabled={busy} onClick={() => onClose(option)}>{option.label}<span className="sr-only">: close as {option.label}</span></button>
            : <span className="record-path__step">{option.label}{isCurrent && <span className="sr-only"> (current)</span>}</span>}
        </li>;
      })}
    </ol>
    <div className="record-path__footer">
      <p className="record-path__status">{current === null
        ? `No ${field.label.toLowerCase()} yet.`
        : closedCurrent ? `Closed as ${closedCurrent.label}${weightOf(closedCurrent)}.` : `${field.label}: ${open[index]?.label ?? current}${weightOf(open[index])}`}</p>
      {editable && next && <Button type="button" size="small" disabled={busy} aria-busy={busy || undefined} onClick={() => onMove(next.value)}>{current === null ? `Start at ${next.label}` : `Mark as ${next.label}`}</Button>}
    </div>
  </nav>;
}

function weightOf(option: PicklistOption | undefined): string {
  return typeof option?.weight === 'number' ? ` (${option.weight}%)` : '';
}
