import React, {useEffect, useMemo, useRef, useState} from 'react';
import {MagnifyingGlass, SquaresFour} from '@phosphor-icons/react';
import {activeContributionFor} from './navigation';
import {ObjectTile} from './ObjectIcon';
import type {NavigationContribution} from './types';

type AppLauncherProps = {
  apps: readonly NavigationContribution[];
  route: string;
  onNavigate: (route: string) => void;
};

/**
 * Lists every feature the workspace has enabled, from the contribution endpoint
 * only. It opens from the rail, filters as you type, and closes on Escape,
 * on an outside click, or when a feature is chosen.
 */
export function AppLauncher({apps, route, onNavigate}: AppLauncherProps) {
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState('');
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    inputRef.current?.focus();
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
  }, [open]);

  const active = activeContributionFor(apps, route);
  const needle = filter.trim().toLocaleLowerCase();
  const visible = useMemo(() => apps.filter(app => !needle || app.label.toLocaleLowerCase().includes(needle)), [apps, needle]);
  const choose = (target: string) => {
    setOpen(false);
    setFilter('');
    onNavigate(target);
  };

  return <div className="app-launcher">
    <button ref={buttonRef} type="button" className="shell-nav__link app-launcher__button" aria-label="Feature launcher" aria-haspopup="dialog" aria-expanded={open} aria-controls="app-launcher-panel" onClick={() => setOpen(value => !value)}>
      <span className="shell-nav__icon"><SquaresFour aria-hidden="true" /></span>
      <span className="shell-nav__label">Features</span>
    </button>
    {open && <div ref={panelRef} id="app-launcher-panel" className="app-launcher__panel" role="dialog" aria-label="Feature launcher">
      <label className="app-launcher__search"><MagnifyingGlass aria-hidden="true" /><span className="sr-only">Find a feature</span><input ref={inputRef} type="search" placeholder="Find a feature…" value={filter} onChange={event => setFilter(event.target.value)} /></label>
      <p className="app-launcher__heading" id="app-launcher-apps">Apps</p>
      {visible.length === 0
        ? <p className="app-launcher__empty" role="status">{apps.length === 0 ? 'No features are enabled for this workspace yet.' : 'No features match.'}</p>
        : <ul className="app-launcher__list" aria-labelledby="app-launcher-apps">
          {visible.map(app => <li key={app.id}>
            <a href={app.route} className="app-launcher__item" aria-current={active?.id === app.id ? 'page' : undefined} onClick={event => { event.preventDefault(); choose(app.route); }}>
              <ObjectTile item={app} />
              <span className="app-launcher__item-text"><strong>{app.label}</strong></span>
            </a>
          </li>)}
        </ul>}
      <div className="app-launcher__footer">
        <span>{apps.length === 1 ? '1 feature enabled' : `${apps.length} features enabled`}</span>
        <a href="/settings" onClick={event => { event.preventDefault(); choose('/settings'); }}>Manage features</a>
      </div>
    </div>}
  </div>;
}
