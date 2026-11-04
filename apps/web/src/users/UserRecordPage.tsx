import {useEffect, useState, type RefObject} from 'react';
import {Alert, Button, Skeleton} from '../components/ui';
import {formatRecordTimestamp} from '../plugin-ui/standard-fields/model';
import {UsersApiError, type UserRecord, type UsersClient} from './api';

export type UserLoad = {status: 'loading'} | {status: 'ready'; user: UserRecord} | {status: 'missing'} | {status: 'unavailable'};

type Props = {
  userId: string;
  onNavigate?: (route: string) => void;
  client: UsersClient;
  headingRef?: RefObject<HTMLHeadingElement | null>;
  onTitle?: (title: string) => void;
};

/** A workspace member's user record, linked from Created by and Last modified by. */
export function UserRecordPage({userId, client, headingRef, onTitle, onNavigate}: Props) {
  const [load, setLoad] = useState<UserLoad>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.getUser(userId, controller.signal)
      .then(user => setLoad({status: 'ready', user}))
      .catch(error => {
        if (controller.signal.aborted) return;
        setLoad({status: error instanceof UsersApiError && error.reason === 'missing' ? 'missing' : 'unavailable'});
      });
    return () => controller.abort();
  }, [client, userId, attempt]);

  const title = load.status === 'ready' ? load.user.name : 'User';
  useEffect(() => onTitle?.(title), [onTitle, title]);

  return <UserRecordView load={load} title={title} headingRef={headingRef} onNavigate={onNavigate} onRetry={() => setAttempt(value => value + 1)} />;
}

/** The user record in each load state; rendered by `UserRecordPage`. */
export function UserRecordView({load, title, headingRef, onRetry, onNavigate}: {load: UserLoad; title: string; headingRef?: RefObject<HTMLHeadingElement | null>; onRetry: () => void; onNavigate?: (route: string) => void}) {
  return <div className="content-canvas user-record">
    <header className="page-header">
      <div><p className="page-header__breadcrumb">Workspace / Users</p><h1 ref={headingRef} tabIndex={-1}>{title}</h1></div>
    </header>
    {load.status === 'loading' && <Skeleton label="Loading user" lines={4} />}
    {load.status === 'missing' && <Alert variant="warning" title="User not available">This person is not a member of this workspace. They may have left it, or the link is out of date.</Alert>}
    {load.status === 'unavailable' && <div className="user-record__state"><Alert variant="danger" title="User records are unavailable">The user could not be loaded. Try again in a moment.</Alert><Button variant="secondary" onClick={() => onRetry()}>Try again</Button></div>}
    {load.status === 'ready' && <section aria-label="User details" className="meta-record-summary">
      <dl className="meta-display-fields">
        <div><dt>Name</dt><dd>{load.user.name}</dd></div>
        <div><dt>Email</dt><dd>{load.user.email ? <a href={`mailto:${load.user.email}`}>{load.user.email}</a> : 'Not provided'}</dd></div>
        <div><dt>Roles</dt><dd>{load.user.roles.length ? load.user.roles.join(', ') : 'No roles'}</dd></div>
        <div><dt>Reports to</dt><dd>{load.user.manager ? <a href={`/users/${encodeURIComponent(load.user.manager.id)}`} onClick={event => { if (!onNavigate) return; event.preventDefault(); onNavigate(`/users/${encodeURIComponent(load.user.manager?.id ?? '')}`); }}>{load.user.manager.name ?? 'Former member'}</a> : 'No one'}</dd></div>
        <div><dt>User since</dt><dd>{formatRecordTimestamp(load.user.createdAt) || 'Not recorded'}</dd></div>
      </dl>
    </section>}
  </div>;
}
