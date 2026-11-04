import React, {useEffect, useState, type RefObject} from 'react';
import {Alert, Button, Select, Skeleton} from '../components/ui';
import {ObjectsApiError, type Member, type ObjectsClient} from './api';
import {failed, type Load} from './ObjectPages';
import './objects.css';

export const membersRoute = '/settings/members';

/**
 * Settings → Members: who each member reports to (ADR 0016). Managers see
 * and edit what their reports own on objects that grant access using
 * hierarchies. Changing a line needs the Manage members permission.
 */
export function MembersPage({client, onNavigate, headingRef}: {client: Pick<ObjectsClient, 'members' | 'setManager'>; onNavigate: (route: string) => void; headingRef?: RefObject<HTMLHeadingElement | null>}) {
  const [load, setLoad] = useState<Load<{members: Member[]; canManage: boolean}>>({status: 'loading'});
  const [attempt, setAttempt] = useState(0);
  const [notice, setNotice] = useState('');
  const [saving, setSaving] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    setLoad({status: 'loading'});
    client.members(controller.signal)
      .then(value => { if (!controller.signal.aborted) setLoad({status: 'ready', value}); })
      .catch(error => { if (!controller.signal.aborted) setLoad(failed(error)); });
    return () => controller.abort();
  }, [client, attempt]);
  useEffect(() => { document.title = 'Members - Limiara'; }, []);

  const change = async (member: Member, managerId: string) => {
    setSaving(member.id);
    try {
      await client.setManager(member.id, managerId || null);
      const manager = load.status === 'ready' ? load.value.members.find(item => item.id === managerId) : undefined;
      setNotice(manager ? `${member.name} now reports to ${manager.name}.` : `${member.name} no longer reports to anyone.`);
      setAttempt(value => value + 1);
    } catch (error) {
      setNotice(error instanceof ObjectsApiError && error.code === 'members.reporting_cycle'
        ? `${member.name} cannot report to someone who already reports to them.`
        : `The reporting line for ${member.name} could not be saved.`);
    } finally {
      setSaving('');
    }
  };

  return <div className="content-canvas object-page">
    <header className="page-header"><div>
      <p className="page-header__breadcrumb"><a href="/settings" onClick={event => { event.preventDefault(); onNavigate('/settings'); }}>Settings</a></p>
      <h1 ref={headingRef} tabIndex={-1}>Members</h1>
      <p>Managers see and edit the records their reports own, on objects that allow it.</p>
    </div></header>
    <div className="sr-only" aria-live="polite">{notice}</div>
    {notice && <Alert variant="info" title={notice} />}
    {load.status === 'loading' && <Skeleton label="Loading members" lines={4} />}
    {load.status !== 'loading' && load.status !== 'ready' && <div className="object-page__state"><Alert variant="danger" title="Members could not be loaded">Try again in a moment.</Alert><Button variant="secondary" onClick={() => setAttempt(value => value + 1)}>Try again</Button></div>}
    {load.status === 'ready' && !load.value.canManage && <Alert variant="info" title="Read-only">Ask an administrator with Manage members to change reporting lines.</Alert>}
    {load.status === 'ready' && <div className="object-table-wrap"><table className="object-table">
      <caption className="sr-only">Workspace members and who they report to</caption>
      <thead><tr><th scope="col">Member</th><th scope="col">Reports to</th></tr></thead>
      <tbody>{load.value.members.map(member => {
        const manager = load.value.members.find(item => item.id === member.managerId);
        return <tr key={member.id}>
          <th scope="row"><a href={`/users/${encodeURIComponent(member.id)}`} onClick={event => { event.preventDefault(); onNavigate(`/users/${encodeURIComponent(member.id)}`); }}>{member.name}</a></th>
          <td>{load.value.canManage
            ? <Select label={`${member.name} reports to`} hideLabel value={member.managerId ?? ''} disabled={saving === member.id} onChange={event => void change(member, event.target.value)}>
              <option value="">No one</option>
              {load.value.members.filter(item => item.id !== member.id).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
            </Select>
            : manager?.name ?? 'No one'}</td>
        </tr>;
      })}</tbody>
    </table></div>}
  </div>;
}
