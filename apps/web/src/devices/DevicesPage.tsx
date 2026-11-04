import React, {useEffect, useState, type RefObject} from 'react';
import {Alert, Badge, Button, EmptyState, Skeleton} from '../components/ui';
import {DevicesApiError, type DeviceRecord, type DevicesClient} from './api';
import '../objects/objects.css';

export const devicesRoute = '/settings/devices';

export type DevicesLoad =
  | {status: 'loading'}
  | {status: 'ready'; devices: DeviceRecord[]}
  | {status: 'denied'}
  | {status: 'unavailable'};

const platformNames: Record<string, string> = {ios: 'iPhone or iPad', android: 'Android', macos: 'Mac', windows: 'Windows', linux: 'Linux'};

function platformName(platform: string): string {
  return platformNames[platform] ?? platform;
}

function when(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(undefined, {dateStyle: 'medium', timeStyle: 'short'});
}

function status(device: DeviceRecord): {label: string; variant: 'success' | 'neutral' | 'danger'} {
  if (!device.revokedAt) return {label: 'Signed in', variant: 'success'};
  if (device.revokedReason === 'signed_out') return {label: 'Signed out', variant: 'neutral'};
  return {label: 'Revoked', variant: 'danger'};
}

/**
 * The devices list as markup, with every state, so tests render it
 * without a browser. Names come from the API and render as text.
 */
export function DevicesView({load, confirming, revoking, notice, headingRef, onNavigate, onRetry, onAsk, onCancel, onRevoke}: {
  load: DevicesLoad;
  confirming?: string;
  revoking?: string;
  notice?: string;
  headingRef?: RefObject<HTMLHeadingElement | null>;
  onNavigate: (route: string) => void;
  onRetry: () => void;
  onAsk: (device: DeviceRecord) => void;
  onCancel: () => void;
  onRevoke: (device: DeviceRecord) => void;
}) {
  return <div className="content-canvas object-page">
    <header className="page-header"><div>
      <p className="page-header__breadcrumb"><a href="/settings" onClick={event => { event.preventDefault(); onNavigate('/settings'); }}>Settings</a></p>
      <h1 ref={headingRef} tabIndex={-1}>Devices</h1>
      <p>Phones and computers signed in to this workspace with the Limiara app. Revoking one signs it out and erases its copy of the workspace the next time it connects.</p>
    </div></header>
    <div className="sr-only" aria-live="polite">{notice}</div>
    {notice && <Alert variant="info" title={notice} />}
    {load.status === 'loading' && <Skeleton label="Loading devices" lines={4} />}
    {load.status === 'denied' && <Alert variant="warning" title="You cannot view these devices">Ask an administrator with Manage devices to review signed-in devices.</Alert>}
    {load.status === 'unavailable' && <div className="object-page__state"><Alert variant="danger" title="Devices could not be loaded">Try again in a moment.</Alert><Button variant="secondary" onClick={onRetry}>Try again</Button></div>}
    {load.status === 'ready' && load.devices.length === 0 && <EmptyState title="No devices yet" description="Devices appear here once someone signs in with the Limiara app." />}
    {load.status === 'ready' && load.devices.length > 0 && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Devices signed in to this workspace</caption>
      <thead><tr><th scope="col">Device</th><th scope="col">Member</th><th scope="col">Last seen</th><th scope="col">Status</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
      <tbody>{load.devices.map(device => {
        const state = status(device);
        return <tr key={device.id}>
          <th scope="row">{device.name}<br /><small>{platformName(device.platform)}{device.current ? ' · this device' : ''}</small></th>
          <td>{device.member.name ?? 'Former member'}</td>
          <td>{when(device.lastSeenAt)}</td>
          <td><Badge variant={state.variant}>{state.label}</Badge>{device.revokedAt && <><br /><small>{when(device.revokedAt)}{device.revokedBy?.name ? ` by ${device.revokedBy.name}` : ''}</small></>}</td>
          <td>{!device.revokedAt && (confirming === device.id
            ? <div role="group" aria-label={`Revoke ${device.name}?`}>
              <Button variant="danger" size="small" loading={revoking === device.id} onClick={() => onRevoke(device)}>Revoke and erase</Button>{' '}
              <Button variant="secondary" size="small" disabled={revoking === device.id} onClick={onCancel}>Cancel</Button>
            </div>
            : <Button variant="secondary" size="small" aria-label={`Revoke ${device.name}`} onClick={() => onAsk(device)}>Revoke</Button>)}</td>
        </tr>;
      })}</tbody>
    </table></div>}
  </div>;
}

/** Settings → Devices (ADR 0029). */
export function DevicesPage({client, onNavigate, headingRef}: {client: DevicesClient; onNavigate: (route: string) => void; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [load, setLoad] = useState<DevicesLoad>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [confirming, setConfirming] = useState<string>();
  const [revoking, setRevoking] = useState<string>();
  const [notice, setNotice] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.list(controller.signal)
      .then(devices => { if (!controller.signal.aborted) setLoad({status: 'ready', devices}); })
      .catch(error => {
        if (controller.signal.aborted) return;
        setLoad({status: error instanceof DevicesApiError && error.reason === 'denied' ? 'denied' : 'unavailable'});
      });
    return () => controller.abort();
  }, [client, attempt]);
  useEffect(() => { document.title = 'Devices - Limiara'; }, []);

  const revoke = async (device: DeviceRecord) => {
    setRevoking(device.id);
    try {
      const revoked = await client.revoke(device.id);
      setLoad(current => current.status === 'ready' ? {status: 'ready', devices: current.devices.map(item => item.id === revoked.id ? revoked : item)} : current);
      setNotice(`${device.name} is revoked. It erases its copy of the workspace the next time it connects.`);
    } catch (error) {
      setNotice(error instanceof DevicesApiError && error.reason === 'denied'
        ? `You cannot revoke ${device.name}.`
        : `${device.name} could not be revoked. Try again in a moment.`);
    } finally {
      setRevoking(undefined);
      setConfirming(undefined);
    }
  };

  return <DevicesView load={load} confirming={confirming} revoking={revoking} notice={notice} headingRef={headingRef} onNavigate={onNavigate}
    onRetry={() => setAttempt(value => value + 1)} onAsk={device => { setNotice(''); setConfirming(device.id); }} onCancel={() => setConfirming(undefined)} onRevoke={device => void revoke(device)} />;
}
