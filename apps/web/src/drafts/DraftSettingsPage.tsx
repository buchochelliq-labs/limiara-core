import {useEffect, useMemo, useState, type RefObject} from 'react';
import {Alert, Button, Skeleton, TextField} from '../components/ui';
import {createDraftSettingsClient, DraftSettingsError, type DraftSettings, type DraftSettingsClient} from './settings';
import './drafts.css';

export const draftSettingsRoute = '/settings/record-drafts';

/**
 * Settings → Record drafts (ADR 0036 §9): how long drafts are kept after
 * their last change, 1 to 90 days, 7 by default. Shown to holders of
 * `objects.manage`; the server checks again.
 */
export function DraftSettingsPage({workspaceId, canManage, client: given, headingRef}: {workspaceId: string; canManage: boolean; client?: DraftSettingsClient; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const client = useMemo(() => given ?? createDraftSettingsClient(workspaceId), [given, workspaceId]);
  const [settings, setSettings] = useState<DraftSettings | null>(null);
  const [load, setLoad] = useState<'loading' | 'ready' | 'failed'>('loading');
  const [days, setDays] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState('');
  const [attempt, setAttempt] = useState(0);
  useEffect(() => { document.title = 'Record drafts - Limiara'; }, []);
  useEffect(() => {
    if (!canManage) return;
    const controller = new AbortController();
    setLoad('loading');
    client.get(controller.signal)
      .then(value => { if (controller.signal.aborted) return; setSettings(value); setDays(String(value.expiryDays)); setLoad('ready'); })
      .catch(() => { if (!controller.signal.aborted) setLoad('failed'); });
    return () => controller.abort();
  }, [client, canManage, attempt]);

  const header = <header className="page-header"><div><p className="page-header__breadcrumb">Settings</p><h1 ref={headingRef} tabIndex={-1}>Record drafts</h1><p>Members’ unsaved changes to records are kept on their own device, encrypted, until they save them, discard them or sign out.</p></div></header>;
  if (!canManage) return <div className="content-canvas">{header}<Alert variant="warning" title="Permission needed">Changing how long drafts are kept needs the permission to manage objects.</Alert></div>;
  if (load === 'loading') return <div className="content-canvas">{header}<Skeleton label="Loading draft settings" lines={2} /></div>;
  if (load === 'failed' || !settings) return <div className="content-canvas">{header}<Alert variant="danger" title="The draft settings could not be loaded. Try again in a moment." /><Button type="button" variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>;

  const value = Number(days);
  const valid = Number.isInteger(value) && value >= settings.minExpiryDays && value <= settings.maxExpiryDays;
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaved('');
    if (!valid) { setError(`Enter a whole number of days from ${settings.minExpiryDays} to ${settings.maxExpiryDays}.`); return; }
    setSaving(true);
    setError('');
    try {
      const next = await client.update(value, settings.version);
      setSettings(next);
      setDays(String(next.expiryDays));
      setSaved(`Drafts are now kept for ${next.expiryDays === 1 ? '1 day' : `${next.expiryDays} days`} after their last change.`);
    } catch (reason) {
      const problem = reason instanceof DraftSettingsError ? reason.problem : 'unavailable';
      setError(problem === 'conflict' ? 'Someone changed this setting since you opened it. Reload the page to see their change.' : problem === 'invalid' ? `Enter a whole number of days from ${settings.minExpiryDays} to ${settings.maxExpiryDays}.` : problem === 'denied' ? 'You do not have permission to change this setting.' : 'The setting could not be saved. Try again in a moment.');
    } finally {
      setSaving(false);
    }
  };
  return <div className="content-canvas">
    {header}
    <form className="drafts-settings" onSubmit={event => void submit(event)} noValidate aria-busy={saving || undefined}>
      {saved && <Alert variant="success" title={saved} />}
      <TextField label="Keep drafts for (days)" type="number" inputMode="numeric" min={settings.minExpiryDays} max={settings.maxExpiryDays} step={1} required value={days} error={error || undefined} description={`From ${settings.minExpiryDays} to ${settings.maxExpiryDays} days after a draft’s last change; ${settings.defaultExpiryDays} by default. A shorter time also removes existing drafts older than it.`} onChange={event => setDays(event.target.value)} />
      <div className="draft-actions"><Button type="submit" loading={saving}>Save</Button></div>
    </form>
  </div>;
}
