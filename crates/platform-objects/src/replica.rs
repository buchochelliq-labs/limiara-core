//! A synced device's replica (ADR 0024 §3). Records the server sends are
//! stored as the server has them, with its version and stamps, and leave no
//! entry in the device's change log. The device's change log then holds only
//! the member's own edits, which is what the device pushes.

use crate::automation::Run;
use crate::service::{stored_index, MutationContext, ObjectService};
use crate::{FieldType, ObjectDefinition, ObjectError, ObjectTx, Record};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

/// A record as the server's record API returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRecord {
    pub public_id: String,
    pub name: String,
    pub owner_id: Option<Uuid>,
    /// Values by field API name.
    pub values: Map<String, Value>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
    pub touched_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl RemoteRecord {
    /// What the replica remembers of this copy, which covers the member's
    /// changes up to `through`.
    fn taken(&self, through: i64) -> crate::RemoteVersion {
        crate::RemoteVersion {
            version: self.version,
            deleted: self.deleted_at.is_some(),
            through,
        }
    }
}

/// What applying a server record did.
#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    Inserted(Record),
    Updated(Record),
    /// The replica already took this server version or a newer one.
    Unchanged(Record),
    /// The record has the member's unpushed edits, so the server's copy was
    /// not stored: the member resolves the two (ADR 0024 §3). The replica
    /// keeps its local record.
    Conflict(Record),
}

impl ObjectService {
    /// Stores a record the server sent in a replica, as the server has it:
    /// its version, owner, stamps and recycle bin state. Only a replica's
    /// engine may (`with_replica`). Nothing is validated, automated or
    /// logged, since the server already did all of it. Fields the replica
    /// does not define are dropped, and formula values are computed on
    /// read.
    ///
    /// `pushed` is the local version a push was made from when the record
    /// is the server's answer to that push: it replaces the local record
    /// unless the member edited it again since. Otherwise the record comes
    /// from a pull, and replaces the local record only when the member has
    /// no unpushed edits to it.
    pub async fn apply_remote(
        &self,
        api_name: &str,
        remote: &RemoteRecord,
        pushed: Option<i64>,
    ) -> Result<Applied, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let existing = tx.record(object.id, &remote.public_id).await?;
        if let Some(current) = &existing {
            let remembered = tx.remote_version(current.id).await?;
            let taken = remembered.map(|taken| taken.version);
            let stale = taken.is_some_and(|taken| remote.version <= taken);
            let unchanged =
                |current: &Record| Applied::Unchanged(Self::project(&object, current.clone()));
            let decision = match pushed {
                // The member edited again after the push: keep the edit, and
                // remember the server version it now builds on.
                Some(pushed) if pushed != current.version => {
                    if !stale {
                        // The edits since still have to go.
                        let through = remembered.map_or(0, |taken| taken.through);
                        tx.set_remote_version(current.id, remote.taken(through))
                            .await?;
                    }
                    Some(Applied::Conflict(Self::project(&object, current.clone())))
                }
                // An answer with the version the replica already took means
                // the server found nothing to change, such as an owner it
                // already had: the local record takes the server's copy.
                Some(_) if taken.is_some_and(|taken| remote.version < taken) => {
                    Some(unchanged(current))
                }
                Some(_) => None,
                None if stale => Some(unchanged(current)),
                // Unpushed edits move the local version past the server's.
                None if taken != Some(current.version) => {
                    Some(Applied::Conflict(Self::project(&object, current.clone())))
                }
                None => None,
            };
            if let Some(decision) = decision {
                tx.commit().await?;
                return Ok(decision);
            }
        }
        let record = self
            .take_remote(tx.as_mut(), &object, existing.as_ref(), remote)
            .await?;
        tx.commit().await?;
        let record = Self::project(&object, record);
        Ok(match existing {
            Some(_) => Applied::Updated(record),
            None => Applied::Inserted(record),
        })
    }

    /// Stores the server's copy of a record as the local one, whatever the
    /// member changed: its version, owner, stamps, values and recycle bin
    /// state. The member's changes so far count as part of the copy.
    async fn take_remote(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        existing: Option<&Record>,
        remote: &RemoteRecord,
    ) -> Result<Record, ObjectError> {
        let record = Record {
            id: existing.map_or_else(Uuid::now_v7, |current| current.id),
            object_id: object.id,
            public_id: remote.public_id.clone(),
            name: remote.name.clone(),
            owner_id: remote.owner_id,
            record_type_id: existing.and_then(|current| current.record_type_id),
            values: defined_values(object, &remote.values),
            version: remote.version,
            created_at: remote.created_at,
            created_by: remote.created_by,
            updated_at: remote.updated_at,
            updated_by: remote.updated_by,
            touched_at: remote.touched_at,
            archived_at: existing.and_then(|current| current.archived_at),
            deleted_at: remote.deleted_at,
        };
        match existing {
            Some(current) => {
                if !tx.update_record(&record, current.version).await? {
                    return Err(ObjectError::VersionConflict);
                }
            }
            None => tx.insert_record(&record).await?,
        }
        // The member's changes so far are in the copy now.
        let (through, _) = tx.change_bounds().await?;
        tx.set_remote_version(record.id, remote.taken(through))
            .await?;
        tx.clear_replica_bases(record.id).await?;
        index_replica_record(tx, object, &record).await?;
        // Records pulled before this one point at it now.
        for waiting in tx.take_pending_references(&record.public_id).await? {
            let Some(child) = tx.record_by_id(waiting).await? else {
                continue;
            };
            let Some(child_object) = tx.object_by_id(child.object_id).await? else {
                continue;
            };
            index_replica_record(tx, &child_object, &child).await?;
        }
        Ok(record)
    }

    /// The server version a replica record last took, which is the version
    /// the member's next edit to it pushes against. `None` for a record the
    /// server has not sent, such as one created on the device.
    pub async fn remote_version(
        &self,
        api_name: &str,
        public_id: &str,
    ) -> Result<Option<i64>, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let version = match tx.record(object.id, public_id).await? {
            Some(record) => tx
                .remote_version(record.id)
                .await?
                .map(|taken| taken.version),
            None => None,
        };
        tx.commit().await?;
        Ok(version)
    }

    /// The live records the replica holds as the server sent them, by
    /// object API name, for the server's revocation set (ADR 0024 §4).
    /// Records made on the device and not pushed yet are left out, since the
    /// server does not know them. Records of objects the replica retired
    /// because the server stopped describing them stay in, so the server's
    /// answer erases them.
    pub async fn replica_manifest(&self) -> Result<BTreeMap<String, Vec<String>>, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let rows = tx.replica_manifest().await?;
        tx.commit().await?;
        let mut manifest: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (object, public_id) in rows {
            manifest.entry(object).or_default().push(public_id);
        }
        Ok(manifest)
    }

    /// Removes records the member can no longer read on the server from the
    /// replica, with their index rows, history and sync state, and returns
    /// how many it held. Nothing is logged, so nothing pushes; edits the
    /// member had not pushed to them go too, since the server would refuse
    /// them. Ids the replica does not hold are passed over.
    pub async fn forget_remote(
        &self,
        api_name: &str,
        public_ids: &[String],
    ) -> Result<usize, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let Some(object) = tx.object_by_api_name(api_name).await? else {
            tx.commit().await?;
            return Ok(0);
        };
        let mut forgotten = 0;
        for public_id in public_ids {
            if let Some(record) = tx.record(object.id, public_id).await? {
                tx.forget_record(record.id).await?;
                forgotten += 1;
            }
        }
        tx.commit().await?;
        Ok(forgotten)
    }
}

/// A reading of the device's monotonic clock (ADR 0024, "Trusted elapsed
/// time"): which boot it belongs to, and the time since that boot,
/// including time asleep. The member cannot set it, unlike the wall clock:
/// on iOS it is `mach_continuous_time` with the kernel's boot time, and on
/// Android `SystemClock.elapsedRealtime` with the boot count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedInstant {
    /// Identifies the boot; it changes at every restart.
    pub boot: String,
    pub since_boot: std::time::Duration,
}

/// Where the replica reads trusted time. Without one, the lease measures
/// wall-clock time against the latest time it has seen.
pub trait TrustedClock: Send + Sync {
    /// The reading now, or `None` when the platform cannot give one.
    fn now(&self) -> Option<TrustedInstant>;
}

impl TrustedInstant {
    /// Time since `earlier`, when both are from the same boot.
    fn since(&self, earlier: &TrustedInstant) -> Option<std::time::Duration> {
        (self.boot == earlier.boot)
            .then(|| self.since_boot.checked_sub(earlier.since_boot))
            .flatten()
    }
}

/// A replica's offline lease (ADR 0024 §4), as the app shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// The limit the server set at sign-in; none for a standalone
    /// workspace, which never locks.
    pub offline_limit_seconds: Option<i64>,
    /// When the device last reached the server.
    pub last_contact_at: Option<DateTime<Utc>>,
    /// When the replica locks unless the device reaches the server first.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the replica refuses the member (`sync.lease_expired`) until
    /// the next contact.
    pub locked: bool,
}

impl Lease {
    /// The lease at `now` by the wall clock, and at `trusted` by the
    /// monotonic clock when the app supplies one. Within the boot of the
    /// last contact, trusted time counts too, so a wall clock turned back
    /// while the app was closed cannot stretch the lease; across a restart
    /// only the wall-clock rule applies.
    pub(crate) fn at(
        lease: &crate::ReplicaLease,
        now: DateTime<Utc>,
        trusted: Option<&TrustedInstant>,
    ) -> Self {
        let by_wall = match (lease.offline_limit_seconds, lease.last_contact_at) {
            (Some(limit), Some(contact)) => {
                contact.checked_add_signed(chrono::Duration::seconds(limit))
            }
            _ => None,
        };
        // The wall-clock rule: time is the latest the replica has seen, so
        // turning the clock back cannot reopen a lease that has run out, and
        // a clock well behind that locks too. Small corrections stay within
        // the tolerance.
        let seen = lease.clock_seen_at.max(lease.last_contact_at);
        let latest = seen.map_or(now, |seen| seen.max(now));
        let turned_back =
            seen.is_some_and(|seen| now + chrono::Duration::seconds(CLOCK_TOLERANCE) < seen);
        let mut locked = by_wall.is_some_and(|expires| latest > expires || turned_back);
        let mut expires_at = by_wall;
        // The trusted rule, checked on its own so a wall-clock correction is
        // never counted twice: within the boot of the last contact, the lease
        // also runs out once the monotonic clock passes the limit.
        let trusted_elapsed = match (trusted, &lease.contact_trusted) {
            (Some(now), Some(contact)) => now.since(contact),
            _ => None,
        };
        if let (Some(limit), Some(elapsed), Some(_)) =
            (lease.offline_limit_seconds, trusted_elapsed, by_wall)
        {
            let elapsed = i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX);
            locked = locked || elapsed > limit;
            // What trusted time leaves, so the app warns on time.
            let left = chrono::Duration::seconds(limit.saturating_sub(elapsed));
            if let Some(by_trusted) = now.checked_add_signed(left) {
                expires_at = expires_at.map(|by_wall| by_wall.min(by_trusted));
            }
        }
        Self {
            offline_limit_seconds: lease.offline_limit_seconds,
            last_contact_at: lease.last_contact_at,
            expires_at,
            locked,
        }
    }

    /// Whether the replica should remember `now` as the latest time it has
    /// seen: it moved on far enough that the write is worth it.
    pub(crate) fn should_note(lease: &crate::ReplicaLease, now: DateTime<Utc>) -> bool {
        lease.offline_limit_seconds.is_some()
            && lease
                .clock_seen_at
                .is_none_or(|seen| now > seen + chrono::Duration::seconds(CLOCK_NOTE_EVERY))
    }
}

/// How far behind the latest time seen the device clock may read before
/// the replica counts it as turned back.
const CLOCK_TOLERANCE: i64 = 300;
/// How often the replica writes down the latest time it has seen.
const CLOCK_NOTE_EVERY: i64 = 60;

impl ObjectService {
    /// Sets the offline limit the server returns with the device's tokens
    /// (`offline_limit_seconds`, ADR 0029). The app passes it at each sign-in
    /// and refresh. Only a replica's engine may.
    pub async fn set_offline_limit(&self, seconds: u64) -> Result<Lease, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        if seconds == 0 {
            return Err(ObjectError::invalid(
                "offline_limit_seconds",
                "sync.offline_limit_invalid",
                "the offline limit is at least one second",
            ));
        }
        let limit = i64::try_from(seconds).unwrap_or(i64::MAX);
        self.change_lease(|lease| lease.offline_limit_seconds = Some(limit))
            .await
    }

    /// Records that the device reached the server, by the device clock,
    /// which renews the lease and unlocks a locked replica. The sync loop
    /// calls it when the server first answers a sync.
    pub async fn record_contact(&self) -> Result<Lease, ObjectError> {
        self.record_contact_at(Utc::now()).await
    }

    /// Records contact at a given time, such as the time the server
    /// answered.
    pub async fn record_contact_at(&self, at: DateTime<Utc>) -> Result<Lease, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let trusted = self.trusted_now();
        self.change_lease(|lease| {
            lease.last_contact_at = Some(at);
            // Contact with the server resets what the clock has shown.
            lease.clock_seen_at = Some(at);
            lease.contact_trusted = trusted;
        })
        .await
    }

    /// The replica's lease now. Only a replica's engine may.
    pub async fn lease(&self) -> Result<Lease, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let lease = tx.replica_lease().await?;
        tx.commit().await?;
        Ok(Lease::at(&lease, Utc::now(), self.trusted_now().as_ref()))
    }

    /// Takes the workspace's members and roles from the server, replacing
    /// what the replica held (ADR 0024, "Offline directory"). Only a
    /// replica's engine may.
    pub async fn apply_directory(
        &self,
        directory: &crate::ReplicaDirectory,
    ) -> Result<(), ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        tx.replace_replica_directory(directory).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The members and roles the replica last took. Only a replica's engine
    /// may.
    pub async fn directory(&self) -> Result<crate::ReplicaDirectory, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let directory = tx.replica_directory().await?;
        tx.commit().await?;
        Ok(directory)
    }

    async fn change_lease(
        &self,
        change: impl FnOnce(&mut crate::ReplicaLease),
    ) -> Result<Lease, ObjectError> {
        let mut tx = self.begin().await?;
        let mut lease = tx.replica_lease().await?;
        change(&mut lease);
        let shown = Lease::at(&lease, Utc::now(), self.trusted_now().as_ref());
        tx.set_replica_lease(lease).await?;
        tx.commit().await?;
        Ok(shown)
    }
}

/// Writes a replica record's index rows. A lookup whose target the replica
/// does not hold keeps its value and is remembered, so the target's arrival
/// indexes it.
async fn index_replica_record(
    tx: &mut dyn crate::ObjectTx,
    object: &ObjectDefinition,
    record: &Record,
) -> Result<(), ObjectError> {
    let mut references = BTreeMap::new();
    for field in object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.field_type.is_reference())
    {
        let Some(target) = record.values.get(&field.api_name).and_then(Value::as_str) else {
            continue;
        };
        match tx.record_by_public_id(target).await? {
            Some(found) => {
                references.insert(field.id, found.id);
            }
            None => tx.add_pending_reference(record.id, target).await?,
        }
    }
    tx.replace_index(
        record.id,
        &stored_index(
            object,
            record,
            &references,
            &crate::formula_fields::stored_formulas(object),
        ),
    )
    .await
}

/// The values of fields the replica defines and that hold data. Formula
/// values are computed on read; roll-up values are stored, so they are kept
/// as the server computed them over details the device may not hold.
fn defined_values(
    object: &ObjectDefinition,
    values: &Map<String, Value>,
) -> BTreeMap<String, Value> {
    values
        .iter()
        .filter(|(key, value)| {
            !value.is_null()
                && object
                    .field(key)
                    .is_some_and(|field| field.field_type != crate::FieldType::Formula)
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Which side's value a member keeps for a field both changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The member's own edit, which pushes next time.
    Local,
    Server,
}

/// A field the member and the server both changed, to different values
/// (ADR 0024 §3). `base` is the value in the server copy both started from.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldConflict {
    /// A field API name, `name` or `owner`.
    pub field: String,
    pub base: Value,
    pub local: Value,
    pub server: Value,
}

/// One side deleted the record while the other edited it. A delete wins
/// only once the member confirms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteConflict {
    /// The member deleted it on the device; the server edited it.
    DeletedHere,
    /// The server deleted it; the member edited it on the device.
    DeletedOnServer,
}

/// How a server copy and the member's unpushed changes to a record combine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Merge {
    /// Fields only the server changed: they take its value.
    pub from_server: Vec<String>,
    /// Fields both changed, for the member to choose.
    pub conflicts: Vec<FieldConflict>,
    pub deletion: Option<DeleteConflict>,
}

impl Merge {
    /// Whether it settles without asking the member.
    pub fn is_automatic(&self) -> bool {
        self.conflicts.is_empty() && self.deletion.is_none()
    }
}

/// The member's choices for a [`Merge`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choices {
    pub fields: BTreeMap<String, Side>,
    /// For a delete conflict: whether the deletion stands.
    pub keep_deletion: Option<bool>,
}

/// What [`ObjectService::merge_remote`] works out, with what `settle`
/// needs to apply it.
struct Plan {
    merge: Merge,
    object: ObjectDefinition,
    local: Record,
    local_values: BTreeMap<String, Value>,
    local_changed: BTreeMap<String, Value>,
    local_deleted: bool,
    server_values: BTreeMap<String, Value>,
}

/// Each merged field's value: `name`, `owner` and the fields that hold
/// data. Formula and roll-up values are the server's to compute.
fn merged_fields(
    object: &ObjectDefinition,
    name: &str,
    owner: Option<Uuid>,
    values: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::from([
        ("name".to_owned(), Value::String(name.to_owned())),
        (
            "owner".to_owned(),
            owner.map_or(Value::Null, |owner| Value::String(owner.to_string())),
        ),
    ]);
    for field in object.fields.iter().filter(|field| {
        field.is_active() && !matches!(field.field_type, FieldType::Formula | FieldType::Rollup)
    }) {
        fields.insert(
            field.api_name.clone(),
            values.get(&field.api_name).cloned().unwrap_or(Value::Null),
        );
    }
    fields
}

/// The fields whose value differs between two versions of a record, with
/// their value before: `name`, `owner` and stored values.
pub(crate) fn changed_fields(before: &Record, after: &Record) -> Vec<(String, Value)> {
    let mut changed = Vec::new();
    if before.name != after.name {
        changed.push(("name".to_owned(), Value::String(before.name.clone())));
    }
    if before.owner_id != after.owner_id {
        changed.push((
            "owner".to_owned(),
            before
                .owner_id
                .map_or(Value::Null, |owner| Value::String(owner.to_string())),
        ));
    }
    let keys: std::collections::BTreeSet<&String> =
        before.values.keys().chain(after.values.keys()).collect();
    for key in keys {
        let old = before.values.get(key);
        if old != after.values.get(key) {
            changed.push((key.clone(), old.cloned().unwrap_or(Value::Null)));
        }
    }
    changed
}

impl ObjectService {
    /// How the server's current copy of a record combines with the member's
    /// unpushed changes, when a push met a version conflict or a pull met
    /// local edits (ADR 0024 §3). Fields only one side changed merge on
    /// their own; fields both changed differently, and a delete against an
    /// edit, are the member's to choose with [`settle`]. Nothing is
    /// written. Only a replica's engine may.
    ///
    /// [`settle`]: ObjectService::settle
    pub async fn merge_remote(
        &self,
        api_name: &str,
        remote: &RemoteRecord,
    ) -> Result<Merge, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let plan = self.plan_merge(tx.as_mut(), api_name, remote).await?;
        tx.commit().await?;
        Ok(plan.merge)
    }

    /// Settles a record with the member's choices: the replica takes the
    /// server's copy, then makes again, as the member's own edits, the
    /// changes the member keeps. They push next time against the server's
    /// version. A conflict without a choice is `sync.choice_missing`.
    pub async fn settle(
        &self,
        api_name: &str,
        remote: &RemoteRecord,
        choices: &Choices,
        context: MutationContext,
    ) -> Result<Record, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut run = Run::default();
        let result = self
            .settle_in(api_name, remote, choices, context, &mut run)
            .await;
        self.keep_failed_runs(run, result).await
    }

    async fn settle_in(
        &self,
        api_name: &str,
        remote: &RemoteRecord,
        choices: &Choices,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Record, ObjectError> {
        let mut tx = self.begin().await?;
        let plan = self.plan_merge(tx.as_mut(), api_name, remote).await?;
        for conflict in &plan.merge.conflicts {
            if !choices.fields.contains_key(&conflict.field) {
                return Err(ObjectError::invalid(
                    conflict.field.clone(),
                    "sync.choice_missing",
                    "choose the device's or the server's value",
                ));
            }
        }
        let keep_deletion = match plan.merge.deletion {
            Some(_) => Some(choices.keep_deletion.ok_or_else(|| {
                ObjectError::invalid(
                    "deleted",
                    "sync.choice_missing",
                    "confirm or undo the deletion",
                )
            })?),
            None => None,
        };
        let server_deleted = remote.deleted_at.is_some();
        // The member's changes that stand.
        let mut kept: BTreeMap<String, Value> = plan
            .local_changed
            .keys()
            .filter(|field| match choices.fields.get(*field) {
                Some(side) => *side == Side::Local,
                None => true,
            })
            .filter_map(|field| {
                plan.local_values
                    .get(field)
                    .map(|value| (field.clone(), value.clone()))
            })
            .filter(|(field, value)| plan.server_values.get(field) != Some(value))
            .collect();
        let deleted = match (plan.merge.deletion, keep_deletion) {
            (Some(DeleteConflict::DeletedHere), Some(keep)) => keep,
            (Some(DeleteConflict::DeletedOnServer), Some(keep)) => {
                if keep {
                    // The server's deletion stands, and the edits go with it.
                    kept.clear();
                }
                keep
            }
            _ => plan.local_deleted,
        };
        let access = self.access(tx.as_mut()).await?;
        let mut record = self
            .take_remote(tx.as_mut(), &plan.object, Some(&plan.local), remote)
            .await?;
        if server_deleted && !deleted {
            record = self
                .undelete_record_tx(
                    tx.as_mut(),
                    &access,
                    api_name,
                    &record.public_id,
                    record.version,
                    context,
                    run,
                )
                .await?
                .1;
        }
        let owner = kept.remove("owner");
        if !kept.is_empty() && !deleted {
            let body: Map<String, Value> = kept.into_iter().collect();
            record = self
                .update_record_tx(
                    tx.as_mut(),
                    &access,
                    api_name,
                    &record.public_id,
                    record.version,
                    &body,
                    context,
                    run,
                )
                .await?
                .1;
        }
        if let Some(owner) = owner.and_then(|owner| owner.as_str().map(str::to_owned)) {
            if !deleted {
                let owner = Uuid::parse_str(&owner).map_err(|_| ObjectError::Invariant)?;
                record = self
                    .change_owner_tx(
                        tx.as_mut(),
                        api_name,
                        &record.public_id,
                        owner,
                        record.version,
                        context,
                        run,
                    )
                    .await?
                    .1;
            }
        }
        if deleted && !server_deleted {
            record = self
                .delete_record_tx(
                    tx.as_mut(),
                    &access,
                    api_name,
                    &record.public_id,
                    record.version,
                    context,
                    run,
                )
                .await?
                .1;
        }
        tx.commit().await?;
        self.with_formula(&plan.object, Self::project(&plan.object, record))
            .await
    }

    async fn plan_merge(
        &self,
        tx: &mut dyn ObjectTx,
        api_name: &str,
        remote: &RemoteRecord,
    ) -> Result<Plan, ObjectError> {
        let object = self.active_object(tx, api_name).await?;
        let local = tx
            .record(object.id, &remote.public_id)
            .await?
            .ok_or(ObjectError::NotFound)?;
        let taken = tx.remote_version(local.id).await?;
        let local_changed = tx.replica_bases(local.id).await?;
        let local_values = merged_fields(&object, &local.name, local.owner_id, &local.values);
        let server_values = merged_fields(
            &object,
            &remote.name,
            remote.owner_id,
            &defined_values(&object, &remote.values),
        );
        let mut merge = Merge::default();
        for (field, server) in &server_values {
            let local = local_values.get(field).unwrap_or(&Value::Null);
            let base = local_changed.get(field).unwrap_or(local);
            if server == base {
                continue;
            }
            if !local_changed.contains_key(field) {
                merge.from_server.push(field.clone());
            } else if server != local {
                merge.conflicts.push(FieldConflict {
                    field: field.clone(),
                    base: base.clone(),
                    local: local.clone(),
                    server: server.clone(),
                });
            }
        }
        let was_deleted = taken.is_some_and(|taken| taken.deleted);
        let local_deleted = local.deleted_at.is_some();
        let server_deleted = remote.deleted_at.is_some();
        let server_edited = taken.is_none_or(|taken| remote.version > taken.version);
        merge.deletion = if local_deleted && !was_deleted && server_edited && !server_deleted {
            Some(DeleteConflict::DeletedHere)
        } else if server_deleted && !was_deleted && !local_deleted && !local_changed.is_empty() {
            Some(DeleteConflict::DeletedOnServer)
        } else {
            None
        };
        Ok(Plan {
            merge,
            object,
            local,
            local_values,
            local_changed,
            local_deleted,
            server_values,
        })
    }
}

#[cfg(test)]
mod lease_tests {
    use super::Lease;
    use crate::ReplicaLease;
    use chrono::{Duration, Utc};

    #[test]
    fn a_clock_turned_back_cannot_reopen_an_expired_lease() {
        let contact = Utc::now() - Duration::hours(3);
        let mut lease = ReplicaLease {
            offline_limit_seconds: Some(3_600),
            last_contact_at: Some(contact),
            clock_seen_at: Some(contact),
            contact_trusted: None,
        };
        // Within the lease, and a small correction backwards, stay open.
        assert!(!Lease::at(&lease, contact + Duration::minutes(30), None).locked);
        lease.clock_seen_at = Some(contact + Duration::minutes(31));
        assert!(!Lease::at(&lease, contact + Duration::minutes(30), None).locked);
        // Once the replica has seen a time past the limit, setting the clock
        // back inside the lease window keeps it locked.
        lease.clock_seen_at = Some(contact + Duration::hours(2));
        assert!(Lease::at(&lease, contact + Duration::minutes(30), None).locked);
        // A clock well behind the latest time seen locks too.
        lease.clock_seen_at = Some(contact + Duration::minutes(40));
        assert!(Lease::at(&lease, contact + Duration::minutes(10), None).locked);
        // Without a limit nothing locks.
        lease.offline_limit_seconds = None;
        assert!(!Lease::at(&lease, contact + Duration::hours(5), None).locked);
    }

    fn at(boot: &str, seconds: u64) -> super::TrustedInstant {
        super::TrustedInstant {
            boot: boot.into(),
            since_boot: std::time::Duration::from_secs(seconds),
        }
    }

    #[test]
    fn trusted_time_counts_within_the_boot_of_the_last_contact() {
        let contact = Utc::now() - Duration::hours(3);
        let lease = ReplicaLease {
            offline_limit_seconds: Some(3_600),
            last_contact_at: Some(contact),
            clock_seen_at: Some(contact),
            contact_trusted: Some(at("boot-1", 1_000)),
        };
        let ten_minutes = contact + Duration::minutes(10);
        // The wall clock says ten minutes, the monotonic clock two hours:
        // the clock was turned back while the app was closed.
        let shown = Lease::at(&lease, ten_minutes, Some(&at("boot-1", 1_000 + 7_200)));
        assert!(shown.locked, "{shown:?}");
        // Both agree on ten minutes: open, and the expiry is by wall time.
        let shown = Lease::at(&lease, ten_minutes, Some(&at("boot-1", 1_000 + 600)));
        assert!(!shown.locked);
        assert_eq!(shown.expires_at, Some(contact + Duration::hours(1)));
        // Trusted time further along than the wall clock brings the expiry
        // forward, so the app warns on time.
        let shown = Lease::at(&lease, ten_minutes, Some(&at("boot-1", 1_000 + 1_800)));
        assert!(!shown.locked);
        assert_eq!(shown.expires_at, Some(ten_minutes + Duration::minutes(30)));
        // After a restart the readings cannot be compared: the wall-clock
        // rule alone applies.
        let shown = Lease::at(&lease, ten_minutes, Some(&at("boot-2", 50)));
        assert!(!shown.locked);
        assert!(
            Lease::at(
                &lease,
                contact + Duration::hours(2),
                Some(&at("boot-2", 50))
            )
            .locked
        );
        // A wall clock corrected back within the tolerance is not counted
        // twice: an hour's lease from 12:00, the latest time seen 12:59, the
        // clock now 12:55 after 59 trusted minutes, is still open.
        let noon = contact;
        let corrected = ReplicaLease {
            clock_seen_at: Some(noon + Duration::minutes(59)),
            ..lease.clone()
        };
        let shown = Lease::at(
            &corrected,
            noon + Duration::minutes(55),
            Some(&at("boot-1", 1_000 + 59 * 60)),
        );
        assert!(!shown.locked, "{shown:?}");
        // Without a trusted clock, nothing changes either.
        assert!(!Lease::at(&lease, ten_minutes, None).locked);
        // Without a limit nothing locks, whatever the clocks say.
        let unlimited = ReplicaLease {
            offline_limit_seconds: None,
            ..lease
        };
        assert!(!Lease::at(&unlimited, ten_minutes, Some(&at("boot-1", 99_999))).locked);
    }

    #[test]
    fn the_latest_time_is_written_down_once_a_minute_at_most() {
        let now = Utc::now();
        let mut lease = ReplicaLease {
            offline_limit_seconds: Some(60),
            last_contact_at: Some(now),
            clock_seen_at: None,
            contact_trusted: None,
        };
        assert!(Lease::should_note(&lease, now));
        lease.clock_seen_at = Some(now);
        assert!(!Lease::should_note(&lease, now + Duration::seconds(30)));
        assert!(Lease::should_note(&lease, now + Duration::seconds(61)));
        lease.offline_limit_seconds = None;
        assert!(!Lease::should_note(&lease, now + Duration::hours(1)));
    }
}
