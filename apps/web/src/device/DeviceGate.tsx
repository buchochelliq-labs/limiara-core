import React, {useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode} from 'react';
import {Alert, Button} from '../components/ui';
import brandMark from '../assets/limiara-mark.svg';
import {createDeviceSessionClient, DeviceSessionError, type DeviceSessionClient, type DeviceStatus} from './session';
import './device.css';

/** How soon before the offline limit the app starts warning. */
const LEASE_WARNING_MS = 2 * 24 * 3600 * 1000;

function when(value: string): string {
  return new Date(value).toLocaleString(undefined, {dateStyle: 'medium', timeStyle: 'short'});
}

export type DeviceViewProps = {
  status: DeviceStatus;
  /** The address typed so far. */
  address: string;
  /** Why the last sign-in request was refused. */
  error?: string;
  busy?: boolean;
  confirmingSignOut?: boolean;
  /** When the lease warning is measured from; the current time in the app. */
  now?: number;
  onAddressChange: (address: string) => void;
  onSignIn: () => void;
  onDemo: () => void;
  onCancel: () => void;
  onAskSignOut: () => void;
  onKeep: () => void;
  onSignOut: () => void;
  children?: ReactNode;
};

/** The device's sign-in states, and the bar above an open workspace. */
export function DeviceSessionView(props: DeviceViewProps) {
  const {status} = props;
  if (status.phase === 'ready' || status.phase === 'demo') {
    return <div className="device-app">
      <DeviceBar {...props} />
      <div className="device-app__workspace">{props.children}</div>
    </div>;
  }
  return <main className="device-gate">
    <section className="device-gate__panel" aria-labelledby="device-gate-title">
      <img className="device-gate__mark" src={brandMark} alt="" />
      {status.phase === 'signed_out' && <SignInForm {...props} />}
      {status.phase === 'browser' && <>
        <h1 id="device-gate-title">Finish signing in in your browser</h1>
        <p>Your browser opened the sign-in page{status.server ? ` for ${status.server}` : ''}. Come back here when it says you can close it.</p>
        <progress aria-label="Waiting for the browser" />
        <Button variant="secondary" onClick={props.onCancel}>Cancel</Button>
      </>}
      {status.phase === 'signing_out' && <>
        <h1 id="device-gate-title">Signing out</h1>
        <p>Deleting this device's copy of {status.workspace ?? 'the workspace'}. You can sign in again in a moment.</p>
        <progress aria-label="Deleting this device's copy" />
      </>}
      {status.phase === 'syncing' && <>
        <h1 id="device-gate-title">Bringing {status.workspace ?? 'your workspace'} to this device</h1>
        <p>Signed in{status.member ? ` as ${status.member}` : ''}. The first sync copies the records you can see, so you can keep working offline.</p>
        <progress aria-label="First sync in progress" />
        {status.syncError && <Alert variant="warning" title="The server could not be reached">The app keeps trying. {status.syncError}</Alert>}
      </>}
    </section>
  </main>;
}

function SignInForm({status, address, error, busy, onAddressChange, onSignIn, onDemo}: DeviceViewProps) {
  const submit = (event: FormEvent) => {
    event.preventDefault();
    onSignIn();
  };
  return <>
    <h1 id="device-gate-title">Sign in to your workspace</h1>
    {status.notice && <Alert variant="info" title={status.notice} />}
    <form className="device-gate__form" onSubmit={submit} noValidate>
      <label htmlFor="device-address">Workspace address</label>
      <input id="device-address" name="address" type="url" inputMode="url" autoComplete="url" required
        placeholder="https://limiara.example.com/workspaces/…" value={address}
        aria-invalid={error ? true : undefined} aria-describedby={error ? 'device-address-hint device-address-error' : 'device-address-hint'}
        onChange={event => onAddressChange(event.target.value)} />
      <p id="device-address-hint" className="device-gate__hint">Find it on the web in Settings → Devices. You sign in in your browser, so this app never sees your password.</p>
      {error && <p id="device-address-error" className="device-gate__error" role="alert">{error}</p>}
      <Button type="submit" loading={busy}>Continue in your browser</Button>
    </form>
    <Button variant="quiet" onClick={onDemo} disabled={busy}>Explore a demonstration workspace</Button>
  </>;
}

function DeviceBar({status, busy, confirmingSignOut, now = Date.now(), onAskSignOut, onKeep, onSignOut}: DeviceViewProps) {
  const lease = status.lease;
  const expires = lease?.expiresAt ? new Date(lease.expiresAt).getTime() : null;
  const demo = status.phase === 'demo';
  return <div className="device-bar" role="region" aria-label="This device">
    <p className="device-bar__summary">
      <strong>{status.workspace ?? 'Workspace'}</strong>
      {demo ? ' · demonstration, nothing is saved' : status.syncError
        ? ' · offline, changes sync when you reconnect'
        : status.lastSyncedAt ? ` · synced ${when(status.lastSyncedAt)}` : ' · syncing'}
    </p>
    {lease?.locked && <Alert variant="danger" title="This device has been offline too long">Connect to the internet so it can sync; the workspace opens again once it reaches the server.</Alert>}
    {!lease?.locked && expires !== null && expires - now < LEASE_WARNING_MS && <Alert variant="warning" title={`Connect by ${when(new Date(expires).toISOString())} to keep working offline`}>After that the workspace locks on this device until it syncs.</Alert>}
    {confirmingSignOut
      ? <div className="device-bar__confirm" role="group" aria-label="Confirm sign-out">
          <span>Sign out and delete this device's copy of the workspace?</span>
          <Button size="small" variant="danger" loading={busy} onClick={onSignOut}>Sign out</Button>
          <Button size="small" variant="secondary" onClick={onKeep}>Keep</Button>
        </div>
      : <Button size="small" variant="secondary" onClick={demo ? onSignOut : onAskSignOut}>{demo ? 'Leave the demonstration' : 'Sign out of this device'}</Button>}
  </div>;
}

const loadingStatus: DeviceStatus = {phase: 'signed_out', notice: null, server: null, workspace: null, member: null, lastSyncedAt: null, syncError: null, lease: null};

/**
 * Shows the app's sign-in until a workspace is open on the device, then the
 * web shell under a bar with its sync state, offline lease and sign-out.
 */
export function DeviceGate({children, client: given}: {children: ReactNode; client?: DeviceSessionClient}) {
  const client = useRef(given ?? createDeviceSessionClient()).current;
  const [status, setStatus] = useState<DeviceStatus>();
  const [address, setAddress] = useState('');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [unavailable, setUnavailable] = useState(false);

  const refresh = useCallback(async (signal?: AbortSignal) => {
    try {
      const next = await client.status(signal);
      setStatus(next);
      setUnavailable(false);
    } catch {
      if (!signal?.aborted) setUnavailable(true);
    }
  }, [client]);

  useEffect(() => {
    const controller = new AbortController();
    void refresh(controller.signal);
    return () => controller.abort();
  }, [refresh]);

  // Quick while the browser or the first sync is under way, slow once open.
  const phase = status?.phase;
  useEffect(() => {
    if (!phase || phase === 'signed_out' || phase === 'demo') return;
    const interval = window.setInterval(() => void refresh(), phase === 'ready' ? 30_000 : 1_000);
    return () => window.clearInterval(interval);
  }, [phase, refresh]);

  const run = async (action: () => Promise<DeviceStatus>) => {
    setBusy(true);
    setError(undefined);
    try {
      setStatus(await action());
    } catch (reason) {
      setError(reason instanceof DeviceSessionError ? reason.message : 'The app could not do that. Try again.');
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };

  if (unavailable) return <main className="device-gate"><section className="device-gate__panel"><Alert variant="danger" title="The app could not start">Close Limiara and open it again.</Alert></section></main>;
  if (!status) return <main className="device-gate" aria-busy="true"><section className="device-gate__panel"><progress aria-label="Opening Limiara" /></section></main>;
  return <DeviceSessionView status={status ?? loadingStatus} address={address} error={error} busy={busy} confirmingSignOut={confirming}
    onAddressChange={setAddress}
    onSignIn={() => {
      if (!address.trim()) { setError('Enter the workspace address from Settings → Devices on the web.'); return; }
      void run(() => client.signIn(address.trim()));
    }}
    onDemo={() => void run(() => client.openDemo())}
    onCancel={() => void run(() => client.signOut())}
    onAskSignOut={() => setConfirming(true)}
    onKeep={() => setConfirming(false)}
    onSignOut={() => void run(() => client.signOut())}>
    {children}
  </DeviceSessionView>;
}
