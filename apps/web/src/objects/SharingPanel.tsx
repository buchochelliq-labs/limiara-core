import React, {useEffect, useState} from 'react';
import {Alert, Badge, Button, Select, Skeleton} from '../components/ui';
import {ObjectsApiError, type Member, type ObjectDescription, type ObjectRecord, type ObjectsClient, type RecordSharing, type Role} from './api';
import {failed, type Load} from './ObjectPages';

type Props = {
  client: ObjectsClient;
  object: ObjectDescription;
  record: ObjectRecord;
  /** Called with the record after its owner changed. */
  onTransferred: (record: ObjectRecord) => void;
};

/**
 * Who owns a record and who else it is shared with (ADR 0016). Owners,
 * their managers and administrators manage shares; changing the owner needs
 * ownership, modify all or the records.transfer permission. Names come from
 * the API and are rendered as text.
 */
export function SharingPanel({client, object, record, onTransferred}: Props) {
  const rights = record.rights;
  const [sharing, setSharing] = useState<Load<RecordSharing>>({status: 'loading'});
  const [members, setMembers] = useState<Member[]>([]);
  const [roles, setRoles] = useState<Role[]>([]);
  const [attempt, setAttempt] = useState(0);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [owner, setOwner] = useState('');
  const [kind, setKind] = useState<'user' | 'role'>('user');
  const [grantee, setGrantee] = useState('');
  const [access, setAccess] = useState<'read' | 'edit'>('read');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!rights?.share && !rights?.transfer) return;
    const controller = new AbortController();
    client.members(controller.signal).then(value => { if (!controller.signal.aborted) setMembers(value.members); }).catch(() => undefined);
    client.roles(controller.signal).then(value => { if (!controller.signal.aborted) setRoles(value); }).catch(() => undefined);
    return () => controller.abort();
  }, [client, rights?.share, rights?.transfer]);

  useEffect(() => {
    if (!rights?.share) return;
    const controller = new AbortController();
    setSharing({status: 'loading'});
    client.getSharing(object.apiName, record.id, controller.signal)
      .then(value => { if (!controller.signal.aborted) setSharing({status: 'ready', value}); })
      .catch(reason => { if (!controller.signal.aborted) setSharing(failed(reason)); });
    return () => controller.abort();
  }, [client, object.apiName, record.id, record.version, rights?.share, attempt]);

  if (!rights?.share && !rights?.transfer) return null;

  const run = async (action: () => Promise<void>, done: string) => {
    setBusy(true);
    setError('');
    try {
      await action();
      setNotice(done);
      setAttempt(value => value + 1);
    } catch (reason) {
      setError(reason instanceof ObjectsApiError && reason.issues[0] ? reason.issues[0].detail : reason instanceof Error ? reason.message : 'That did not work.');
    } finally {
      setBusy(false);
    }
  };

  const transfer = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!owner) return;
    const name = members.find(member => member.id === owner)?.name ?? 'the new owner';
    await run(async () => {
      const moved = await client.changeOwner(object.apiName, record.id, owner, record.version);
      setOwner('');
      onTransferred(moved);
    }, `${record.name} now belongs to ${name}. Its manual shares were removed.`);
  };

  const share = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!grantee) return;
    const name = (kind === 'user' ? members : roles).find(item => item.id === grantee)?.name ?? 'them';
    await run(async () => {
      await client.share(object.apiName, record.id, kind, grantee, access);
      setGrantee('');
    }, `Shared with ${name} (${access === 'edit' ? 'read and edit' : 'read only'}).`);
  };

  const ownerId = record.owner?.id;
  const candidates = kind === 'user' ? members.filter(member => member.id !== ownerId) : roles;

  return <section className="object-section" aria-labelledby="sharing-heading">
    <h2 id="sharing-heading">Sharing</h2>
    <p>Owned by <strong>{record.owner?.name ?? 'no one'}</strong>. {object.sharingModel === 'private' ? 'This object is private: others see this record only through their manager line or a share.' : object.sharingModel === 'read' ? 'Everyone with access to this object can see it; edits need ownership or a share.' : 'Everyone with access to this object can see and edit it.'}</p>
    <div className="sr-only" aria-live="polite">{notice}</div>
    {notice && <Alert variant="success" title={notice} />}
    {error && <Alert variant="danger" title={error} />}
    {rights.transfer && <form className="object-toolbar" onSubmit={event => void transfer(event)} aria-label="Transfer ownership">
      <Select label="Change owner" value={owner} onChange={event => setOwner(event.target.value)}>
        <option value="">Choose a member</option>
        {members.filter(member => member.id !== ownerId).map(member => <option key={member.id} value={member.id}>{member.name}</option>)}
      </Select>
      <Button type="submit" variant="secondary" disabled={!owner || busy}>Transfer</Button>
    </form>}
    {rights.share && <>
      {sharing.status === 'loading' && <Skeleton label="Loading shares" lines={2} />}
      {sharing.status !== 'loading' && sharing.status !== 'ready' && <Alert variant="danger" title="Shares could not be loaded" />}
      {sharing.status === 'ready' && (sharing.value.shares.length === 0
        ? object.sharingModel !== 'read_write' && <p className="object-value--empty">Not shared with anyone yet.</p>
        : <div className="object-table-wrap"><table className="object-table">
          <caption className="sr-only">Shared with</caption>
          <thead><tr><th scope="col">Shared with</th><th scope="col">Access</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
          <tbody>{sharing.value.shares.map(item => <tr key={`${item.granteeKind}:${item.granteeId}`}>
            <th scope="row">{item.granteeName ?? 'Unknown'} <Badge>{item.granteeKind === 'role' ? 'Role' : 'Member'}</Badge></th>
            <td>{item.access === 'edit' ? 'Read and edit' : 'Read only'}</td>
            <td><Button size="small" variant="quiet" disabled={busy} aria-label={`Stop sharing with ${item.granteeName ?? 'this grantee'}`} onClick={() => void run(() => client.unshare(object.apiName, record.id, item.granteeKind, item.granteeId), `No longer shared with ${item.granteeName ?? 'them'}.`)}>Remove</Button></td>
          </tr>)}</tbody>
        </table></div>)}
      {object.sharingModel === 'read_write'
        ? <p className="object-value--empty">Everyone who can open {object.pluralLabel.toLowerCase()} can already edit this record, so there is nothing to share. Administrators can make the object private or read only in the Object Manager.</p>
        : <form className="object-toolbar" onSubmit={event => void share(event)} aria-label="Share this record">
        <Select label="Share with" value={kind} onChange={event => { setKind(event.target.value as 'user' | 'role'); setGrantee(''); }}>
          <option value="user">A member</option>
          <option value="role">Everyone in a role</option>
        </Select>
        <Select label={kind === 'user' ? 'Member' : 'Role'} value={grantee} onChange={event => setGrantee(event.target.value)}>
          <option value="">Choose…</option>
          {candidates.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
        </Select>
        <Select label="Access" value={access} onChange={event => setAccess(event.target.value as 'read' | 'edit')}>
          <option value="read">Read only</option>
          <option value="edit">Read and edit</option>
        </Select>
        <Button type="submit" disabled={!grantee || busy}>Share</Button>
      </form>}
    </>}
  </section>;
}
