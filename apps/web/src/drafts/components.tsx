import {useEffect, useRef, useState} from 'react';
import {Alert, Button} from '../components/ui';
import {draftAnnouncement, draftStatusText, type DraftState} from './autosave';
import {draftTitle} from './model';
import {expiresSoon, type DraftNotices, type StoredDraft} from './types';
import './drafts.css';

export function formatDraftTime(at: number, now = Date.now()): string {
  const date = new Date(at);
  const today = new Date(now);
  const time = date.toLocaleTimeString('en-GB', {hour: '2-digit', minute: '2-digit'});
  if (date.toDateString() === today.toDateString()) return time;
  return `${date.toLocaleDateString('en-GB', {day: 'numeric', month: 'short'})}, ${time}`;
}

/** "3 drafts expired", "1 draft could not be restored": shown once, then reset (ADR 0036 §8). */
export function noticeLines(notices: DraftNotices): string[] {
  const lines: string[] = [];
  if (notices.expired > 0) lines.push(notices.expired === 1 ? '1 draft expired' : `${notices.expired} drafts expired`);
  if (notices.dropped > 0) lines.push(notices.dropped === 1 ? '1 draft could not be restored' : `${notices.dropped} drafts could not be restored`);
  return lines;
}

/**
 * The form footer's draft state as text (ADR 0036 §8). The polite live
 * region holds only the state ("Draft saved", a problem), so a screen
 * reader hears the first save and changes of state, never each save while
 * the member types; the time beside it is for sighted members.
 */
export function DraftStatus({state, unavailable = false, maxDrafts = 50}: {state: DraftState; unavailable?: boolean; maxDrafts?: number}) {
  const current = unavailable ? 'unavailable' as const : state;
  const text = current !== 'unavailable' && current.kind === 'failed' && current.reason === 'limit'
    ? `Draft not saved: you have ${maxDrafts} drafts. Discard some from Drafts.`
    : draftStatusText(current);
  const announcement = draftAnnouncement(current, maxDrafts);
  const failed = unavailable || (state.kind === 'failed');
  return <div className={failed ? 'draft-status draft-status--problem' : 'draft-status'}>
    <span className="sr-only" role="status" aria-live="polite">{announcement}</span>
    <span aria-hidden="true">{text}</span>
  </div>;
}

/** Offered above the fields when a form opens with a draft; until the member chooses, nothing is overwritten. */
export function RestorePrompt({draft, notices, expiryDays, busy = false, onRestore, onDiscard}: {draft: StoredDraft; notices: DraftNotices; expiryDays: number; busy?: boolean; onRestore: () => void; onDiscard: () => void}) {
  const restoreRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { restoreRef.current?.focus(); }, []);
  const soon = expiresSoon(draft.meta.changedAt, expiryDays);
  return <section className="draft-prompt" aria-labelledby={`draft-prompt-${draft.meta.id}`}>
    <p id={`draft-prompt-${draft.meta.id}`}><strong>You have an unsaved draft from {formatDraftTime(draft.meta.changedAt)}.</strong>{soon && <> Expires tomorrow.</>}</p>
    {noticeLines(notices).map(line => <p key={line} className="draft-prompt__notice">{line}</p>)}
    <div className="draft-actions">
      <Button type="button" ref={restoreRef} size="small" loading={busy} onClick={onRestore}>Restore</Button>
      <Button type="button" size="small" variant="secondary" disabled={busy} onClick={onDiscard}>Discard</Button>
    </div>
  </section>;
}

/** Cancel or Escape on a form with a saved draft (ADR 0036 §8); keeping is the default, as closing an email draft keeps it. */
export function CancelChoice({onKeep, onDiscard, onBack}: {onKeep: () => void; onDiscard: () => void; onBack: () => void}) {
  const keepRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { keepRef.current?.focus(); }, []);
  return <div className="draft-cancel" role="group" aria-labelledby="draft-cancel-title">
    <p id="draft-cancel-title"><strong>You have an unsaved draft.</strong> Keep it to finish later?</p>
    <div className="draft-actions">
      <Button type="button" ref={keepRef} size="small" onClick={onKeep}>Keep draft</Button>
      <Button type="button" size="small" variant="danger" onClick={onDiscard}>Discard draft</Button>
      <Button type="button" size="small" variant="quiet" onClick={onBack}>Back to form</Button>
    </div>
  </div>;
}

/** New on an object with unsaved new-record drafts: continue one, or start another (ADR 0036 §8). */
export function CreateDraftChooser({drafts, expiryDays, onContinue, onStartNew}: {drafts: StoredDraft[]; expiryDays: number; onContinue: (draft: StoredDraft) => void; onStartNew: () => void}) {
  const headingId = 'draft-chooser-title';
  return <section className="draft-chooser" aria-labelledby={headingId}>
    <p id={headingId}><strong>{drafts.length === 1 ? '1 unsaved draft' : `${drafts.length} unsaved drafts`}</strong></p>
    <ul>
      {drafts.map(draft => <li key={draft.meta.id}>
        <span>{draftTitle(draft.document)}, {formatDraftTime(draft.meta.changedAt)}{expiresSoon(draft.meta.changedAt, expiryDays) && <>, expires tomorrow</>}</span>
        <Button type="button" size="small" variant="secondary" onClick={() => onContinue(draft)} aria-label={`Continue ${draftTitle(draft.document)}`}>Continue</Button>
      </li>)}
    </ul>
    <Button type="button" size="small" onClick={onStartNew}>Start new</Button>
  </section>;
}

/** The form's draft alerts: restore problems and the fields a restore left out. */
export function DraftMessages({message, dropped}: {message: {tone: 'info' | 'warning'; text: string} | null; dropped: string[]}) {
  return <>
    {message && <Alert variant={message.tone} title={message.text} />}
    {dropped.length > 0 && <Alert variant="warning" title="Your draft was restored">
      <ul>{dropped.map(line => <li key={line}>{line}</li>)}</ul>
    </Alert>}
  </>;
}

/** A blocked draft (unavailable, or with conflicts in this slice): it can be kept or discarded. */
export function BlockedDraft({onKeep, onDiscard}: {onKeep: () => void; onDiscard: () => void}) {
  const [busy, setBusy] = useState(false);
  return <div className="draft-actions">
    <Button type="button" size="small" variant="secondary" onClick={onKeep} disabled={busy}>Keep draft</Button>
    <Button type="button" size="small" variant="danger" loading={busy} onClick={() => { setBusy(true); onDiscard(); }}>Discard draft</Button>
  </div>;
}
