//! The sync loop (ADR 0024 §3–4, "Continual sync").

use crate::transport::{StreamEvent, SyncTransport};
use crate::wire::{problem_code, remote_object, remote_record};
use crate::SyncError;
use futures_util::StreamExt;
use platform_objects::{
    Applied, Choices, DefinitionsApplied, Merge, MutationContext, ObjectError, ObjectService,
    PushAnswer, PushBatch, PushReport, Record, RemoteRecord,
};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

/// Changes read per page.
const PAGE: u16 = 200;
/// Record ids per revocation request (`sync.manifest_too_large` above).
const MANIFEST_MAX: usize = 1_000;
/// Composite requests one sync sends at most, so a queue that keeps
/// growing while it drains cannot hold the loop.
const PUSH_ROUNDS_MAX: usize = 40;
const VERSION_CONFLICT: &str = "records.version_conflict";
/// How long the foreground loop waits after a local change before it
/// pushes, so a burst of edits goes as one composite request.
const PUSH_PAUSE: Duration = Duration::from_millis(750);
/// How often the foreground loop confirms contact with the server while
/// the stream stays open, so a connected device's offline lease never runs
/// out (ADR 0024 §4).
const RENEW_EVERY: Duration = Duration::from_secs(5 * 60);

/// A record the member and the server both changed in ways that need the
/// member's choice. The app shows `merge` and calls [`SyncClient::settle`].
#[derive(Debug, Clone, PartialEq)]
pub struct PendingConflict {
    pub object: String,
    pub public_id: String,
    pub merge: Merge,
}

/// What one sync did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncReport {
    /// The definitions taken, when the server's had moved.
    pub definitions: Option<DefinitionsApplied>,
    /// Records forgotten because the member can no longer read them.
    pub revoked: usize,
    /// Each composite request's outcome.
    pub pushed: Vec<PushReport>,
    /// Server records stored by the pull.
    pub pulled: usize,
    /// Whether the pull read every record again: on the first sync, and
    /// when the stored position had expired.
    pub resynced: bool,
    /// Records whose server copy merged with the member's edits on its own,
    /// as `(object, public id)`; the merged edits push against the server's
    /// version.
    pub merged: Vec<(String, String)>,
    /// Records waiting for the member's choice.
    pub conflicts: Vec<PendingConflict>,
    /// The members and roles taken into the offline directory, as
    /// `(members, roles)`.
    pub directory: Option<(usize, usize)>,
}

/// Clears a background sync's deadline when dropped.
struct ClearDeadline<'a, T: SyncTransport>(&'a SyncClient<T>);

impl<T: SyncTransport> Drop for ClearDeadline<'_, T> {
    fn drop(&mut self) {
        self.0.set_deadline(None);
    }
}

/// Drives one replica's sync with its server workspace.
pub struct SyncClient<T: SyncTransport> {
    engine: ObjectService,
    transport: T,
    /// Whether the next sync asks for the revocation set: after start and
    /// after each reconnect.
    revoke: AtomicBool,
    /// Signalled by [`SyncClient::local_change`].
    changed: Notify,
    push_pause: Duration,
    renew_every: Duration,
    /// When a background sync must stop ([`SyncClient::wake`]).
    deadline: std::sync::Mutex<Option<tokio::time::Instant>>,
    /// Held by each step that talks to the server or writes the replica, so
    /// a background wake and the foreground loop never interleave: neither
    /// pushes the same queued change twice, and the wake's deadline cuts off
    /// only the wake's own requests.
    step: tokio::sync::Mutex<()>,
}

impl<T: SyncTransport> SyncClient<T> {
    /// `engine` is the device's replica engine (`with_replica`, and
    /// `with_client_ids` so offline records keep their ids).
    pub fn new(engine: ObjectService, transport: T) -> Self {
        Self {
            engine,
            transport,
            revoke: AtomicBool::new(true),
            changed: Notify::new(),
            push_pause: PUSH_PAUSE,
            renew_every: RENEW_EVERY,
            deadline: std::sync::Mutex::new(None),
            step: tokio::sync::Mutex::new(()),
        }
    }

    /// How long [`SyncClient::follow`] waits after a local change before it
    /// pushes.
    /// How often [`SyncClient::follow`] confirms contact with the server
    /// while the stream stays open. Keep it well under the workspace's
    /// offline limit.
    pub fn with_renew_every(mut self, every: Duration) -> Self {
        self.renew_every = every;
        self
    }

    pub fn with_push_pause(mut self, pause: Duration) -> Self {
        self.push_pause = pause;
        self
    }

    /// The app calls this after each local write, so the foreground loop
    /// pushes it after a short pause.
    pub fn local_change(&self) {
        self.changed.notify_one();
    }

    pub fn engine(&self) -> &ObjectService {
        &self.engine
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The app calls this when the device comes back online, so the next
    /// sync also asks which records the member lost access to meanwhile.
    pub fn reconnected(&self) {
        self.revoke.store(true, Ordering::Relaxed);
    }

    /// Brings the replica in line with the server: definitions when they
    /// moved, the revocation set after a reconnect, then the member's
    /// queued changes, then the server's.
    pub async fn sync_once(&self, context: MutationContext) -> Result<SyncReport, SyncError> {
        let _step = self.step.lock().await;
        self.sync_steps(context).await
    }

    async fn sync_steps(&self, context: MutationContext) -> Result<SyncReport, SyncError> {
        let mut report = SyncReport::default();
        let positions = self.engine.sync_positions().await?;
        // Without a position, the page starts at the oldest change kept,
        // which never expires.
        let probe = self.net(self.transport.changes(None, 1)).await?;
        // The server accepted the device's token: the offline lease renews,
        // which unlocks a replica that was locked (ADR 0024 §4).
        self.engine.record_contact().await?;
        let mut metadata_version = positions.metadata_version;
        if metadata_version.is_none_or(|known| known < probe.metadata_version) {
            // The first sync reads every record below anyway.
            let backfill = positions.pulled.is_some();
            self.refresh_definitions(backfill, &mut report, context)
                .await?;
            metadata_version = Some(probe.metadata_version);
        }
        // Members and roles change without the metadata version moving, so
        // every sync takes them again: two small reads.
        let directory = self.net(self.transport.directory()).await?;
        self.engine.apply_directory(&directory).await?;
        report.directory = Some((directory.members.len(), directory.roles.len()));
        if self.revoke.load(Ordering::Relaxed) {
            report.revoked = self.revocations().await?;
            self.revoke.store(false, Ordering::Relaxed);
        }
        self.push(&mut report, context).await?;
        let pulled = match positions.pulled {
            Some(pulled) => Some(pulled),
            None => {
                // The first sync reads every record, then follows the log
                // from where it stood before.
                self.resync(&mut report, context).await?;
                self.engine
                    .record_pull(probe.head, metadata_version.unwrap_or(0))
                    .await?;
                Some(probe.head)
            }
        };
        self.pull(pulled, metadata_version, &mut report, context)
            .await?;
        if !report.merged.is_empty() {
            // Merged edits push against the server's version straight away.
            self.push(&mut report, context).await?;
        }
        Ok(report)
    }

    /// One short sync for a device the OS woke in the background, after a
    /// push notification or a scheduled refresh (ADR 0024, "Continual
    /// sync"). Once `budget` runs out it stops between steps, or cuts off
    /// the request in flight, with [`SyncError::OutOfTime`]: everything
    /// stored before then stays stored, since positions move only once what
    /// they cover is, and the next sync carries on from there. A step that
    /// writes to the replica always finishes.
    pub async fn wake(
        &self,
        context: MutationContext,
        budget: Duration,
    ) -> Result<SyncReport, SyncError> {
        // The budget counts from now: waiting for the foreground loop's step
        // in flight spends it too. No foreground step starts until the wake
        // is done, so the deadline is the wake's alone.
        let deadline = tokio::time::Instant::now() + budget;
        let Ok(_step) = tokio::time::timeout_at(deadline, self.step.lock()).await else {
            return Err(SyncError::OutOfTime);
        };
        self.set_deadline(Some(deadline));
        // Cleared however the sync ends, including when the app drops it,
        // so later syncs run without a budget.
        let _clear = ClearDeadline(self);
        self.sync_steps(context).await
    }

    fn set_deadline(&self, deadline: Option<tokio::time::Instant>) {
        if let Ok(mut slot) = self.deadline.lock() {
            *slot = deadline;
        }
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline.lock().ok().and_then(|slot| *slot)
    }

    /// Stops a background sync between steps once its time is up. Steps
    /// that write to the replica always finish: cutting one off part-way
    /// could leave its transaction holding the database.
    fn check_time(&self) -> Result<(), SyncError> {
        match self.deadline() {
            Some(deadline) if tokio::time::Instant::now() >= deadline => Err(SyncError::OutOfTime),
            _ => Ok(()),
        }
    }

    /// A request to the server, cut off when a background sync's time is
    /// up. Requests hold no replica transaction, so stopping one is safe.
    async fn net<R>(
        &self,
        request: impl Future<Output = Result<R, SyncError>>,
    ) -> Result<R, SyncError> {
        self.check_time()?;
        match self.deadline() {
            Some(deadline) => tokio::time::timeout_at(deadline, request)
                .await
                .unwrap_or(Err(SyncError::OutOfTime)),
            None => request.await,
        }
    }

    /// Keeps the replica in line while the app is in the foreground and
    /// online, until `stop` completes or an error the app must handle:
    /// - a sync first, then `GET /objects/changes/stream` held open from the
    ///   stored position, storing each change as it arrives and taking new
    ///   definitions when a `metadata` event announces them;
    /// - local changes ([`SyncClient::local_change`]) push after a short
    ///   pause, so a burst of edits goes as one request;
    /// - when the server ends the stream (every 15 minutes) or its position
    ///   expires, another sync runs, which also asks for the revocation set,
    ///   and the stream resumes.
    ///
    /// `observe` gets what each step did, including conflicts waiting for
    /// the member. A step that started (a sync, a push, storing a change)
    /// finishes before `stop` is honoured. A dropped connection ends the loop with
    /// [`SyncError::Transport`]; the app calls it again once online.
    pub async fn follow(
        &self,
        context: MutationContext,
        stop: impl Future<Output = ()>,
        mut observe: impl FnMut(&SyncReport),
    ) -> Result<(), SyncError> {
        tokio::pin!(stop);
        loop {
            // A sync that started finishes, so its report is never lost;
            // `stop` is checked between steps and while waiting.
            let report = self.sync_once(context).await?;
            observe(&report);
            let positions = self.engine.sync_positions().await?;
            let mut metadata_version = positions.metadata_version;
            let mut stream = match self.transport.stream(positions.pulled).await {
                Ok(stream) => stream,
                // The next sync reads every record again.
                Err(SyncError::PositionExpired) => continue,
                Err(error) => return Err(error),
            };
            let mut renew_at = tokio::time::Instant::now() + self.renew_every;
            loop {
                tokio::select! {
                    biased;
                    () = &mut stop => return Ok(()),
                    () = tokio::time::sleep_until(renew_at) => {
                        let _step = self.step.lock().await;
                        // A stream that stays open says nothing about the
                        // device's token, so contact is confirmed with a
                        // request of its own.
                        self.net(self.transport.changes(None, 1)).await?;
                        self.engine.record_contact().await?;
                        renew_at = tokio::time::Instant::now() + self.renew_every;
                    }
                    () = self.changed.notified() => {
                        // Edits made during the pause go with this push.
                        tokio::select! {
                            biased;
                            () = &mut stop => return Ok(()),
                            () = tokio::time::sleep(self.push_pause) => {}
                        }
                        let _step = self.step.lock().await;
                        let mut report = SyncReport::default();
                        self.push(&mut report, context).await?;
                        observe(&report);
                    }
                    event = stream.next() => match event {
                        // The server ended the stream: sync again, which
                        // re-checks access, then resume.
                        None | Some(Ok(StreamEvent::Expired)) => {
                            self.reconnected();
                            break;
                        }
                        Some(Err(SyncError::PositionExpired)) => break,
                        Some(Err(error)) => return Err(error),
                        Some(Ok(StreamEvent::Metadata(version))) => {
                            let _step = self.step.lock().await;
                            if metadata_version.is_none_or(|known| known < version) {
                                let mut report = SyncReport::default();
                                self.refresh_definitions(true, &mut report, context).await?;
                                if self.revoke.swap(false, Ordering::Relaxed) {
                                    report.revoked = self.revocations().await?;
                                }
                                metadata_version = Some(version);
                                let pulled = self.engine.sync_positions().await?.pulled;
                                self.engine
                                    .record_pull(pulled.unwrap_or(0), version)
                                    .await?;
                                observe(&report);
                            }
                        }
                        Some(Ok(StreamEvent::Change(entry))) => {
                            let _step = self.step.lock().await;
                            let mut report = SyncReport::default();
                            self.pull_record(&entry.object, &entry.record, &mut report, context)
                                .await?;
                            self.engine
                                .record_pull(entry.position, metadata_version.unwrap_or(0))
                                .await?;
                            if !report.merged.is_empty() {
                                self.push(&mut report, context).await?;
                            }
                            observe(&report);
                        }
                    },
                }
            }
        }
    }

    /// Settles a record with the member's choices, against the server's
    /// copy as it is now. The changes the member keeps push with the next
    /// sync.
    pub async fn settle(
        &self,
        api_name: &str,
        public_id: &str,
        choices: &Choices,
        context: MutationContext,
    ) -> Result<Record, SyncError> {
        let Some(body) = self.net(self.transport.record(api_name, public_id)).await? else {
            // The member can no longer read it on the server.
            self.engine
                .forget_remote(api_name, &[public_id.to_owned()])
                .await?;
            return Err(SyncError::Engine(ObjectError::NotFound));
        };
        let remote = remote_record(&body)?;
        Ok(self
            .engine
            .settle(api_name, &remote, choices, context)
            .await?)
    }

    /// Takes the server's definitions. Objects the member can read only
    /// now have records the log already passed while they were hidden, and
    /// a permission change leaves no log entry, so with `backfill` their
    /// records are read in full. The revocation set is asked for next, since
    /// access may have narrowed too.
    async fn refresh_definitions(
        &self,
        backfill: bool,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<(), SyncError> {
        let before = self.held_objects().await?;
        let applied = self.take_definitions().await?;
        if backfill {
            for object in self.held_objects().await?.difference(&before) {
                self.resync_object(object, report, context).await?;
            }
        }
        report.definitions = Some(applied);
        self.revoke.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn held_objects(&self) -> Result<BTreeSet<String>, SyncError> {
        Ok(self
            .engine
            .list_objects()
            .await?
            .into_iter()
            .map(|(object, _)| object.api_name)
            .collect())
    }

    async fn take_definitions(&self) -> Result<DefinitionsApplied, SyncError> {
        let mut objects = Vec::new();
        for listed in self.net(self.transport.objects()).await? {
            let Some(api_name) = listed.get("api_name").and_then(|name| name.as_str()) else {
                return Err(SyncError::protocol("an object has `api_name`"));
            };
            // Gone since the list was read: it is retired with the rest.
            let Some(described) = self.net(self.transport.describe(api_name)).await? else {
                continue;
            };
            let rules = self.net(self.transport.validation_rules(api_name)).await?;
            let actions = self.net(self.transport.quick_actions(api_name)).await?;
            objects.push(remote_object(
                &described,
                rules.as_deref(),
                actions.as_deref(),
            )?);
        }
        Ok(self.engine.apply_remote_definitions(&objects).await?)
    }

    async fn revocations(&self) -> Result<usize, SyncError> {
        let manifest = self.engine.replica_manifest().await?;
        let mut forgotten = 0;
        let mut chunk: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut size = 0;
        // Owned pairs: a borrowed iterator held across the awaits below
        // would keep the sync's future from being `Send`.
        let ids: Vec<(String, String)> = manifest
            .into_iter()
            .flat_map(|(object, ids)| ids.into_iter().map(move |id| (object.clone(), id)))
            .collect();
        let mut pending = ids.into_iter().peekable();
        while let Some((object, id)) = pending.next() {
            chunk.entry(object).or_default().push(id);
            size += 1;
            if size == MANIFEST_MAX || pending.peek().is_none() {
                for (object, revoked) in self.net(self.transport.revocations(&chunk)).await? {
                    forgotten += self.engine.forget_remote(&object, &revoked).await?;
                }
                chunk.clear();
                size = 0;
            }
        }
        Ok(forgotten)
    }

    /// Sends the queue until it is empty, or until a request stores
    /// nothing and settles nothing, so a change the server keeps refusing
    /// waits for the member instead of looping.
    async fn push(
        &self,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<(), SyncError> {
        for _ in 0..PUSH_ROUNDS_MAX {
            self.check_time()?;
            let batch = self.engine.push_queue().await?;
            if batch.is_empty() {
                return Ok(());
            }
            let answers = self.send(&batch).await?;
            let taken = self
                .engine
                .take_push_answers(&batch, &answers, context)
                .await?;
            let mut progress = !taken.stored.is_empty();
            for (object, public_id, code) in taken.failed.clone() {
                if code != VERSION_CONFLICT {
                    continue;
                }
                if let Some(body) = self.net(self.transport.record(&object, &public_id)).await? {
                    let remote = remote_record(&body)?;
                    progress |= self.resolve(&object, &remote, report, context).await?;
                }
            }
            report.pushed.push(taken);
            if !progress {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn send(&self, batch: &PushBatch) -> Result<BTreeMap<String, PushAnswer>, SyncError> {
        let mut answers = BTreeMap::new();
        for result in self.net(self.transport.composite(&batch.request())).await? {
            let answer = if (200..300).contains(&result.status) {
                PushAnswer::Done(remote_record(&result.body)?)
            } else {
                PushAnswer::Failed {
                    status: result.status,
                    code: problem_code(&result.body),
                }
            };
            answers.insert(result.reference, answer);
        }
        Ok(answers)
    }

    /// Merges a server copy with the member's unpushed edits: on its own
    /// when no field needs a choice, otherwise reported for the member.
    /// Whether it merged.
    async fn resolve(
        &self,
        object: &str,
        remote: &RemoteRecord,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<bool, SyncError> {
        let merge = self.engine.merge_remote(object, remote).await?;
        if merge.is_automatic() {
            self.engine
                .settle(object, remote, &Choices::default(), context)
                .await?;
            report
                .merged
                .push((object.to_owned(), remote.public_id.clone()));
            report
                .conflicts
                .retain(|pending| pending.public_id != remote.public_id);
            return Ok(true);
        }
        report
            .conflicts
            .retain(|pending| pending.public_id != remote.public_id);
        report.conflicts.push(PendingConflict {
            object: object.to_owned(),
            public_id: remote.public_id.clone(),
            merge,
        });
        Ok(false)
    }

    /// Follows the change log from `after`, taking new definitions first
    /// whenever a page was read under newer ones.
    async fn pull(
        &self,
        mut after: Option<i64>,
        mut metadata_version: Option<i64>,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<(), SyncError> {
        loop {
            self.check_time()?;
            let page = match self.net(self.transport.changes(after, PAGE)).await {
                Ok(page) => page,
                Err(SyncError::PositionExpired) => {
                    let probe = self.net(self.transport.changes(None, 1)).await?;
                    // Records are read again under the definitions they
                    // were made under, and their version is stored with
                    // the pull.
                    if metadata_version.is_none_or(|known| known < probe.metadata_version) {
                        self.refresh_definitions(false, report, context).await?;
                    }
                    self.resync(report, context).await?;
                    self.engine
                        .record_pull(probe.head, probe.metadata_version)
                        .await?;
                    after = Some(probe.head);
                    metadata_version = Some(probe.metadata_version);
                    continue;
                }
                Err(error) => return Err(error),
            };
            if metadata_version.is_none_or(|known| known < page.metadata_version) {
                self.refresh_definitions(true, report, context).await?;
                metadata_version = Some(page.metadata_version);
            }
            // Each record once a page, as the server has it now.
            let mut seen = BTreeSet::new();
            for entry in &page.entries {
                if seen.insert((entry.object.as_str(), entry.record.as_str())) {
                    self.pull_record(&entry.object, &entry.record, report, context)
                        .await?;
                }
            }
            self.engine
                .record_pull(page.next_after, page.metadata_version)
                .await?;
            if page.next_after >= page.head || Some(page.next_after) == after {
                return Ok(());
            }
            after = Some(page.next_after);
        }
    }

    async fn pull_record(
        &self,
        object: &str,
        public_id: &str,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<(), SyncError> {
        self.check_time()?;
        // Not visible: the revocation set removes it if the device holds it.
        let Some(body) = self.net(self.transport.record(object, public_id)).await? else {
            return Ok(());
        };
        self.store(object, &remote_record(&body)?, report, context)
            .await
    }

    async fn store(
        &self,
        object: &str,
        remote: &RemoteRecord,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<(), SyncError> {
        match self.engine.apply_remote(object, remote, None).await {
            Ok(Applied::Inserted(_) | Applied::Updated(_)) => report.pulled += 1,
            Ok(Applied::Unchanged(_)) => {}
            Ok(Applied::Conflict(_)) => {
                self.resolve(object, remote, report, context).await?;
            }
            // An object the replica does not hold, such as one the member
            // cannot read.
            Err(ObjectError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    /// Reads every record of every object the replica holds, tombstones
    /// included.
    async fn resync(
        &self,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<(), SyncError> {
        report.resynced = true;
        for object in self.held_objects().await? {
            self.resync_object(&object, report, context).await?;
        }
        Ok(())
    }

    /// Reads every record of one object, tombstones included.
    async fn resync_object(
        &self,
        object: &str,
        report: &mut SyncReport,
        context: MutationContext,
    ) -> Result<(), SyncError> {
        let mut cursor: Option<String> = None;
        loop {
            self.check_time()?;
            let page = self
                .net(self.transport.records_page(object, cursor.as_deref()))
                .await?;
            for body in &page.records {
                self.store(object, &remote_record(body)?, report, context)
                    .await?;
            }
            match page.next_cursor {
                Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
                _ => return Ok(()),
            }
        }
    }
}
