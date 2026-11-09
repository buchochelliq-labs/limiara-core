import {useEffect, useRef, useState} from 'react';
import {Plus} from '@phosphor-icons/react';
import type {ObjectDescription, ObjectsClient, ObjectSummary} from './api';
import {failed, recordRoute, RecordModal, type Load} from './ObjectPages';

type Navigate = (route: string) => void;

/**
 * The objects the global New menu offers: engine objects in navigation the
 * member may create, by label. Plugin resources join through ADR 0011's
 * `global-new` slot instead (ADR 0032 §1).
 */
export function creatable(objects: readonly ObjectSummary[]): ObjectSummary[] {
  return objects.filter(object => object.inNavigation && object.access.create && object.origin !== 'plugin')
    .sort((a, b) => a.label.localeCompare(b.label));
}

/**
 * The shell's New menu: choosing an object opens its record form over the
 * current page. The list loads when the menu first opens.
 */
export function GlobalNew({client, onNavigate}: {client: ObjectsClient; onNavigate: Navigate}) {
  const [open, setOpen] = useState(false);
  const [objects, setObjects] = useState<Load<ObjectSummary[]> | null>(null);
  const [chosen, setChosen] = useState<Load<ObjectDescription> | null>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open || objects) return;
    const controller = new AbortController();
    setObjects({status: 'loading'});
    client.listObjects(controller.signal)
      .then(value => { if (!controller.signal.aborted) setObjects({status: 'ready', value: creatable(value.objects)}); })
      .catch(reason => { if (!controller.signal.aborted) setObjects(failed(reason)); });
    return () => controller.abort();
  }, [client, open, objects]);

  useEffect(() => {
    if (!open) return;
    panelRef.current?.querySelector<HTMLElement>('button, [role="status"]')?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setOpen(false); buttonRef.current?.focus(); }
    };
    const onPointer = (event: MouseEvent) => {
      const target = event.target as Node;
      if (panelRef.current?.contains(target) || buttonRef.current?.contains(target)) return;
      setOpen(false);
    };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onPointer);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onPointer);
    };
  }, [open, objects]);

  const choose = (apiName: string) => {
    setOpen(false);
    setChosen({status: 'loading'});
    client.describe(apiName)
      .then(value => setChosen({status: 'ready', value}))
      .catch(reason => setChosen(failed(reason)));
  };
  // Focus returns to New when the form closes, since the menu item is gone.
  const close = () => { setChosen(null); buttonRef.current?.focus(); };

  return <div className="global-new">
    <button ref={buttonRef} type="button" className="ui-button ui-button--secondary ui-button--small global-new__button" aria-haspopup="true" aria-expanded={open} aria-controls="global-new-panel" onClick={() => {
      // Reopening after a failed load tries again.
      if (!open && objects && objects.status !== 'ready' && objects.status !== 'loading') setObjects(null);
      setChosen(current => current?.status === 'ready' ? current : null);
      setOpen(value => !value);
    }}>
      <span><Plus aria-hidden="true" weight="bold" />New</span>
    </button>
    {open && <div ref={panelRef} id="global-new-panel" className="global-new__panel" role="group" aria-labelledby="global-new-heading">
      <p className="global-new__heading" id="global-new-heading">Create a record</p>
      {objects?.status === 'ready' && objects.value.length > 0 && <ul className="global-new__list">
        {objects.value.map(object => <li key={object.apiName}><button type="button" className="global-new__item" onClick={() => choose(object.apiName)}>{object.label}</button></li>)}
      </ul>}
      {objects?.status === 'ready' && objects.value.length === 0 && <p className="global-new__note" role="status" tabIndex={-1}>There is nothing you can create here yet.</p>}
      {(!objects || objects.status === 'loading') && <p className="global-new__note" role="status" tabIndex={-1}>Loading objects…</p>}
      {objects && objects.status !== 'ready' && objects.status !== 'loading' && <p className="global-new__note" role="status" tabIndex={-1}>The objects could not be loaded. Close the menu and try again.</p>}
    </div>}
    {chosen?.status === 'ready' && <RecordModal client={client} object={chosen.value} onNavigate={target => { setChosen(null); onNavigate(target); }} onClose={close} onSaved={(saved, next) => {
      if (next === 'close') { setChosen(null); onNavigate(recordRoute(chosen.value.apiName, saved.id)); }
    }} />}
    {chosen && chosen.status !== 'ready' && chosen.status !== 'loading' && <p className="global-new__error" role="alert">The form could not be opened. Try again.</p>}
  </div>;
}
