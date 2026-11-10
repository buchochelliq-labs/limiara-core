//! The app's sign-in and sync lifecycle (ADR 0029, ADR 0024 §4), the same
//! on every platform. The native shell supplies the parts that differ:
//!
//! - a [`Browser`] that opens the system browser (the desktop's default
//!   browser, `ASWebAuthenticationSession` on iOS, Custom Tabs on Android);
//! - [`Redirects`] that receive the sign-in's redirect: [`LoopbackRedirects`]
//!   on desktop, the app's private-use scheme on phones;
//! - a [`SessionVault`] and [`DatabaseKeys`] in the platform keychain.
//!
//! The [`Controller`] signs the member in through the server's device
//! flow, builds the replica's engine for them from `GET /devices/current`,
//! runs the first sync and then follows the workspace. Sign-out, and a
//! server that refuses the device, delete the replica and its key. There
//! is no password anywhere.

use crate::{device_databases, Bridge, DatabaseKeys, DeviceHost, Member, Workspace};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use platform_objects::store::sqlite::SqliteObjectStore;
use platform_objects::{MutationContext, ObjectCaller, ObjectService, ObjectStore};
use platform_storage::sqlite::SqliteTenantDatabases;
pub use platform_sync::PushProvider;
use platform_sync::{
    AuthError, DeviceSession, HttpTransport, Registration, SignIn, SyncClient, SyncError,
    SyncReport, TokenClient, TokenSink, Tokens,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use url::Url;
use uuid::Uuid;

/// How long the app waits for the member to finish in the browser.
const SIGN_IN_WAIT: Duration = Duration::from_secs(10 * 60);
/// Retry delays after a sync that could not reach the server.
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(5 * 60);
/// How often the app rereads the member's profile, so a change to their
/// permissions or roles reaches the replica's engine.
const PROFILE_EVERY: Duration = Duration::from_secs(15 * 60);
/// How long sign-out waits for the server before it wipes anyway.
const SIGN_OUT_WAIT: Duration = Duration::from_secs(10);
/// The largest redirect request the loopback listener reads.
const REDIRECT_MAX: usize = 8 * 1024;

/// The member profile the app stores, from `GET /devices/current`.
pub use platform_sync::{DeviceProfile, ProfileMember, ProfileWorkspace};

const WIPED: &str =
    "This device no longer has access to the workspace, so its copy was deleted. Sign in again to continue.";

// --- Ports -------------------------------------------------------------------

/// What the app keeps between launches, in the platform keychain: the
/// server, the rotating refresh token and the member's profile, so the
/// replica opens offline.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub server: String,
    pub refresh_token: String,
    pub offline_limit_seconds: u64,
    pub profile: DeviceProfile,
}

impl fmt::Debug for StoredSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredSession")
            .field("server", &self.server)
            .field("offline_limit_seconds", &self.offline_limit_seconds)
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the keychain is unavailable: {0}")]
pub struct VaultError(pub String);

/// The platform keychain's entry for the signed-in session.
#[async_trait]
pub trait SessionVault: Send + Sync {
    async fn load(&self) -> Result<Option<StoredSession>, VaultError>;
    async fn save(&self, session: &StoredSession) -> Result<(), VaultError>;
    async fn delete(&self) -> Result<(), VaultError>;
}

/// A vault in memory, for tests: nothing survives the process.
#[derive(Default)]
pub struct MemoryVault(Mutex<Option<StoredSession>>);

#[async_trait]
impl SessionVault for MemoryVault {
    async fn load(&self) -> Result<Option<StoredSession>, VaultError> {
        Ok(self.0.lock().map_err(poisoned)?.clone())
    }

    async fn save(&self, session: &StoredSession) -> Result<(), VaultError> {
        *self.0.lock().map_err(poisoned)? = Some(session.clone());
        Ok(())
    }

    async fn delete(&self) -> Result<(), VaultError> {
        *self.0.lock().map_err(poisoned)? = None;
        Ok(())
    }
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> VaultError {
    VaultError("the vault's lock was poisoned".into())
}

/// Opens the system browser at the sign-in URL.
pub trait Browser: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

/// The redirect the browser delivers at the end of a sign-in.
pub type PendingRedirect = Pin<Box<dyn Future<Output = Result<Url, String>> + Send>>;

/// Where the app receives its redirect. [`Redirects::prepare`] starts
/// listening before the browser opens, so the redirect cannot arrive first.
#[async_trait]
pub trait Redirects: Send + Sync {
    async fn prepare(&self) -> Result<PendingRedirect, String>;
}

/// A desktop app's redirect: plain HTTP to a loopback address (RFC 8252
/// §7.3), on the port registered in `platform.devices.redirect_uris`.
pub struct LoopbackRedirects {
    address: SocketAddr,
    path: String,
}

impl LoopbackRedirects {
    /// `redirect_uri` is the registered URI, such as
    /// `http://127.0.0.1:47185/callback`.
    pub fn new(redirect_uri: &str) -> Result<Self, String> {
        let url = Url::parse(redirect_uri).map_err(|error| error.to_string())?;
        let host = match url.host() {
            Some(url::Host::Ipv4(ip)) if ip.is_loopback() => std::net::IpAddr::V4(ip),
            Some(url::Host::Ipv6(ip)) if ip.is_loopback() => std::net::IpAddr::V6(ip),
            _ => return Err("the redirect URI is not a loopback address".into()),
        };
        let port = url
            .port()
            .ok_or_else(|| "the redirect URI names no port".to_owned())?;
        if url.scheme() != "http" {
            return Err("a loopback redirect URI is plain HTTP".into());
        }
        Ok(Self {
            address: SocketAddr::new(host, port),
            path: url.path().to_owned(),
        })
    }
}

#[async_trait]
impl Redirects for LoopbackRedirects {
    async fn prepare(&self) -> Result<PendingRedirect, String> {
        let listener = tokio::net::TcpListener::bind(self.address)
            .await
            .map_err(|error| format!("port {} is in use: {error}", self.address.port()))?;
        let address = self.address;
        let path = self.path.clone();
        Ok(Box::pin(async move {
            loop {
                let (mut stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
                let target = match read_request_target(&mut stream).await {
                    Some(target) => target,
                    None => continue,
                };
                let Ok(url) = Url::parse(&format!("http://{address}{target}")) else {
                    continue;
                };
                if url.path() != path {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                        .await;
                    continue;
                }
                let page = "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Limiara</title>\
                    <p>You can close this tab and return to the Limiara app.</p></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\n\
                     referrer-policy: no-referrer\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
                    page.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
                return Ok(url);
            }
        }))
    }
}

/// The target of a `GET` request line, such as `/callback?code=…`.
async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 || buffer.len() + read > REDIRECT_MAX {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let head = std::str::from_utf8(&buffer).ok()?;
    let mut line = head.lines().next()?.split_whitespace();
    (line.next()? == "GET").then_some(())?;
    let target = line.next()?;
    target.starts_with('/').then(|| target.to_owned())
}

// --- Status ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No workspace on the device.
    #[default]
    SignedOut,
    /// Waiting for the member to sign in in the browser.
    Browser,
    /// Signed in; the first sync is bringing the workspace down.
    Syncing,
    /// The replica is open.
    Ready,
    /// The demonstration workspace is open, with no server.
    Demo,
    /// The workspace is closed and its replica, key and session are being
    /// deleted; signing in waits until they are gone.
    SigningOut,
}

/// The offline lease, for the shell's warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeaseStatus {
    pub expires_at: Option<DateTime<Utc>>,
    pub locked: bool,
}

/// What the shell shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    pub phase: Phase,
    /// Why the device is signed out, or what just happened.
    pub notice: Option<String>,
    pub server: Option<String>,
    pub workspace: Option<String>,
    pub member: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    /// Why the last sync did not finish; the app keeps retrying.
    pub sync_error: Option<String>,
    pub lease: Option<LeaseStatus>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignInError {
    #[error("enter the workspace address from Settings → Devices on the web")]
    InvalidAddress,
    #[error("a workspace is already open on this device")]
    Busy,
}

/// The server and workspace from a workspace address, such as
/// `https://limiara.example.com/workspaces/018f…`.
pub fn parse_address(address: &str) -> Result<(String, Uuid), SignInError> {
    let url = Url::parse(address.trim()).map_err(|_| SignInError::InvalidAddress)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(SignInError::InvalidAddress);
    }
    let workspace = url
        .path_segments()
        .and_then(|segments| {
            segments
                .filter_map(|segment| Uuid::parse_str(segment).ok())
                .next_back()
        })
        .ok_or(SignInError::InvalidAddress)?;
    Ok((url.origin().ascii_serialization(), workspace))
}

// --- Controller --------------------------------------------------------------

/// What the app tells the server about itself.
#[derive(Debug, Clone)]
pub struct AppSettings {
    /// This build's registered redirect URI.
    pub redirect_uri: String,
    /// Shown in the member's device list.
    pub device_name: String,
    /// `ios`, `android` or `desktop`.
    pub platform: String,
}

/// The platform's parts.
pub struct Services {
    pub vault: Arc<dyn SessionVault>,
    pub keys: Arc<dyn DatabaseKeys>,
    pub browser: Arc<dyn Browser>,
    pub redirects: Arc<dyn Redirects>,
}

struct Active {
    generation: u64,
    workspace: Uuid,
    files: Arc<SqliteTenantDatabases>,
    engine: ObjectService,
    session: Arc<DeviceSession>,
    sink: Arc<Sink>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    /// Rereads the member's profile ([`Controller::watch_profile`]).
    profile_task: Option<JoinHandle<()>>,
    /// Background wakes in flight ([`Controller::wake`]), stopped with the
    /// rest before the workspace's storage is deleted.
    wakes: Vec<JoinHandle<()>>,
}

/// Stores each rotated refresh token and passes the offline limit to the
/// replica.
struct Sink {
    vault: Arc<dyn SessionVault>,
    engine: ObjectService,
    stored: tokio::sync::Mutex<StoredSession>,
    /// Set before the workspace is forgotten, so a refresh still in flight
    /// cannot store the session again after sign-out or a wipe.
    closed: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl TokenSink for Sink {
    async fn rotated(&self, tokens: &Tokens) -> Result<(), AuthError> {
        let mut stored = self.stored.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        stored.refresh_token = tokens.refresh_token.clone();
        stored.offline_limit_seconds = tokens.offline_limit_seconds;
        let limit = self
            .engine
            .set_offline_limit(tokens.offline_limit_seconds)
            .await;
        self.vault
            .save(&stored)
            .await
            .map_err(|error| AuthError::Storage(error.0))?;
        limit
            .map(|_| ())
            .map_err(|error| AuthError::Storage(error.to_string()))
    }
}

/// The app's one workspace: signed out, signing in, or open.
pub struct Controller {
    directory: PathBuf,
    settings: AppSettings,
    services: Services,
    status: Mutex<Status>,
    bridge: RwLock<Option<Bridge>>,
    sync: RwLock<Option<Arc<SyncClient<HttpTransport>>>>,
    active: tokio::sync::Mutex<Option<Active>>,
    attempt: Mutex<Option<JoinHandle<()>>>,
    generation: AtomicU64,
    /// The bridge the first sync opens, rebuilt when the member's profile
    /// changes before then.
    prepared: Mutex<Option<Bridge>>,
    /// The platform's push token ([`Controller::register_push`]), sent to
    /// the server whenever a workspace opens.
    push: Mutex<Option<(PushProvider, String)>>,
}

/// What a background wake did ([`Controller::wake`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeOutcome {
    /// No workspace is open; nothing to sync.
    NoWorkspace,
    /// The sync finished within its budget.
    Synced,
    /// The budget ran out; what was stored stays stored, and the next sync
    /// carries on.
    OutOfTime,
    /// The server asked for the device to be wiped, and it was.
    Wiped,
}

fn message(error: &AuthError) -> String {
    match error {
        AuthError::Refused(_) => "The sign-in was cancelled in the browser.".into(),
        AuthError::StateMismatch => "The browser returned another sign-in. Try again.".into(),
        AuthError::InvalidGrant(detail) => format!("The server refused the sign-in: {detail}."),
        AuthError::InvalidServer => "That address is not a Limiara server.".into(),
        AuthError::Transport(_) => {
            "The server could not be reached. Check the address and your connection.".into()
        }
        AuthError::WipeRequired => WIPED.into(),
        other => format!("The sign-in failed: {other}."),
    }
}

impl Controller {
    /// `directory` holds the device's encrypted workspace files.
    pub fn new(
        directory: impl Into<PathBuf>,
        settings: AppSettings,
        services: Services,
    ) -> Arc<Self> {
        Arc::new(Self {
            directory: directory.into(),
            settings,
            services,
            status: Mutex::new(Status::default()),
            bridge: RwLock::new(None),
            sync: RwLock::new(None),
            active: tokio::sync::Mutex::new(None),
            attempt: Mutex::new(None),
            generation: AtomicU64::new(0),
            prepared: Mutex::new(None),
            push: Mutex::new(None),
        })
    }

    fn update(&self, change: impl FnOnce(&mut Status)) {
        if let Ok(mut status) = self.status.lock() {
            change(&mut status);
        }
    }

    fn phase(&self) -> Phase {
        self.status
            .lock()
            .map(|status| status.phase)
            .unwrap_or_default()
    }

    fn set_bridge(&self, bridge: Option<Bridge>) {
        if let Ok(mut slot) = self.bridge.write() {
            *slot = bridge;
        }
    }

    /// Closes the open workspace at once and holds the app in
    /// [`Phase::SigningOut`] while its storage is deleted.
    fn closing(&self) {
        self.set_bridge(None);
        if let Ok(mut sync) = self.sync.write() {
            *sync = None;
        }
        if let Ok(mut prepared) = self.prepared.lock() {
            *prepared = None;
        }
        self.update(|status| {
            *status = Status {
                phase: Phase::SigningOut,
                workspace: status.workspace.take(),
                ..Status::default()
            }
        });
    }

    fn signed_out(&self, notice: Option<String>) {
        self.set_bridge(None);
        if let Ok(mut sync) = self.sync.write() {
            *sync = None;
        }
        self.update(|status| {
            *status = Status {
                notice,
                ..Status::default()
            }
        });
    }

    /// The bridge over the open workspace, if one is open.
    pub fn bridge(&self) -> Option<Bridge> {
        self.bridge.read().ok().and_then(|bridge| bridge.clone())
    }

    /// The shell calls this after each write through the bridge, so the
    /// foreground loop pushes it.
    pub fn local_change(&self) {
        if let Some(sync) = self.sync.read().ok().and_then(|sync| sync.clone()) {
            sync.local_change();
        }
    }

    /// What the shell shows, with the lease read from the replica.
    pub async fn status(&self) -> Status {
        let mut status = self
            .status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default();
        if status.phase == Phase::Ready {
            let engine = self
                .active
                .lock()
                .await
                .as_ref()
                .map(|active| active.engine.clone());
            if let Some(engine) = engine {
                status.lease = engine.lease().await.ok().map(|lease| LeaseStatus {
                    expires_at: lease.expires_at,
                    locked: lease.locked,
                });
            }
        }
        status
    }

    /// At launch: reopens the workspace the keychain holds a session for,
    /// before any network, and starts following it.
    pub async fn resume(self: &Arc<Self>) {
        match self.services.vault.load().await {
            Ok(Some(stored)) => {
                let files = device_databases(&self.directory, self.services.keys.clone());
                if let Err(notice) = self.activate(stored, files, None).await {
                    self.signed_out(Some(notice));
                }
            }
            Ok(None) => {}
            Err(error) => self.signed_out(Some(format!(
                "The saved sign-in could not be read: {error}. Sign in again."
            ))),
        }
    }

    /// Starts signing in to the workspace at `address` in the system
    /// browser. [`Controller::status`] follows it.
    pub fn start_sign_in(self: &Arc<Self>, address: &str) -> Result<(), SignInError> {
        let (server, workspace) = parse_address(address)?;
        let mut attempt = self.attempt.lock().map_err(|_| SignInError::Busy)?;
        {
            let mut status = self.status.lock().map_err(|_| SignInError::Busy)?;
            if status.phase != Phase::SignedOut {
                return Err(SignInError::Busy);
            }
            *status = Status {
                phase: Phase::Browser,
                server: Some(server.clone()),
                ..Status::default()
            };
        }
        let controller = self.clone();
        *attempt = Some(tokio::spawn(async move {
            if let Err(notice) = controller.sign_in(server, workspace).await {
                controller.signed_out(Some(notice));
            }
        }));
        Ok(())
    }

    /// Stops a sign-in that is waiting for the browser.
    pub fn cancel_sign_in(&self) {
        if !matches!(self.phase(), Phase::Browser) {
            return;
        }
        if let Some(attempt) = self
            .attempt
            .lock()
            .ok()
            .and_then(|mut attempt| attempt.take())
        {
            attempt.abort();
        }
        self.signed_out(None);
    }

    async fn sign_in(self: &Arc<Self>, server: String, workspace: Uuid) -> Result<(), String> {
        let sign_in =
            SignIn::start(&server, &self.settings.redirect_uri).map_err(|error| message(&error))?;
        let pending = self
            .services
            .redirects
            .prepare()
            .await
            .map_err(|error| format!("The app could not receive the sign-in ({error})."))?;
        self.services
            .browser
            .open(&sign_in.browser_url())
            .map_err(|error| format!("The browser could not be opened ({error})."))?;
        let redirect = tokio::time::timeout(SIGN_IN_WAIT, pending)
            .await
            .map_err(|_| "The sign-in timed out. Try again.".to_owned())?
            .map_err(|error| format!("The sign-in did not come back ({error})."))?;
        let code = sign_in.code(&redirect).map_err(|error| message(&error))?;
        let client = TokenClient::new(&server).map_err(|error| message(&error))?;
        let tokens = client
            .redeem(
                &sign_in,
                &code,
                &Registration {
                    workspace,
                    device_name: self.settings.device_name.clone(),
                    platform: self.settings.platform.clone(),
                },
            )
            .await
            .map_err(|error| message(&error))?;
        self.update(|status| status.phase = Phase::Syncing);
        let profile = client
            .profile(&tokens.access_token, workspace)
            .await
            .map_err(|error| message(&error))?;
        let stored = StoredSession {
            server: server.clone(),
            refresh_token: tokens.refresh_token.clone(),
            offline_limit_seconds: tokens.offline_limit_seconds,
            profile,
        };
        if let Err(error) = self.services.vault.save(&stored).await {
            // A device the app cannot keep a token for is signed out again.
            let _ = client.sign_out(&tokens.access_token, workspace).await;
            return Err(format!("The sign-in could not be saved: {error}."));
        }
        // Signing in starts from an empty replica (ADR 0029 §4).
        let files = device_databases(&self.directory, self.services.keys.clone());
        files
            .wipe(workspace)
            .await
            .map_err(|_| "The device's storage is unavailable.".to_owned())?;
        self.activate(stored, files, Some(tokens)).await
    }

    /// Builds the replica's engine and bridge for the stored member and
    /// starts the sync loop. A resumed workspace opens at once; a new one
    /// opens after its first sync.
    async fn activate(
        self: &Arc<Self>,
        stored: StoredSession,
        files: Arc<SqliteTenantDatabases>,
        tokens: Option<Tokens>,
    ) -> Result<(), String> {
        let profile = stored.profile.clone();
        let workspace = profile.workspace.id;
        let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(files.clone()));
        let engine = replica_engine(store.clone(), &profile);
        engine
            .set_offline_limit(stored.offline_limit_seconds)
            .await
            .map_err(|_| "The device's copy of the workspace could not be opened.".to_owned())?;
        let client = TokenClient::new(&stored.server).map_err(|error| message(&error))?;
        let sink = Arc::new(Sink {
            vault: self.services.vault.clone(),
            engine: engine.clone(),
            stored: tokio::sync::Mutex::new(stored.clone()),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        let session = Arc::new(match &tokens {
            Some(tokens) => DeviceSession::new(client, tokens, sink.clone()),
            None => DeviceSession::resume(
                client,
                workspace,
                stored.refresh_token.clone(),
                sink.clone(),
            ),
        });
        let transport = HttpTransport::new(stored.server.clone(), workspace, session.clone())
            .map_err(|error| error.to_string())?;
        let sync = Arc::new(SyncClient::new(engine.clone(), transport));
        let bridge = bridge_for(engine.clone(), &profile);
        if let Ok(mut prepared) = self.prepared.lock() {
            *prepared = Some(bridge.clone());
        }
        let resumed = tokens.is_none();
        self.update(|status| {
            *status = Status {
                phase: if resumed {
                    Phase::Ready
                } else {
                    Phase::Syncing
                },
                server: Some(stored.server.clone()),
                workspace: Some(profile.workspace.name.clone()),
                member: Some(profile.member.display_name.clone()),
                ..Status::default()
            }
        });
        if resumed {
            self.set_bridge(Some(bridge.clone()));
        }
        if let Ok(mut slot) = self.sync.write() {
            *slot = Some(sync.clone());
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (stop, stopped) = watch::channel(false);
        let mut active = self.active.lock().await;
        let task =
            tokio::spawn(
                self.clone()
                    .run_sync(generation, sync, session.clone(), stopped.clone()),
            );
        let profile_task = tokio::spawn(self.clone().watch_profile(
            generation,
            session.clone(),
            sink.clone(),
            store,
            stopped,
        ));
        *active = Some(Active {
            generation,
            workspace,
            files,
            engine,
            session,
            sink,
            stop,
            task: Some(task),
            profile_task: Some(profile_task),
            wakes: Vec::new(),
        });
        drop(active);
        // A device signed in again is a new device to the server: it gets
        // the platform's token too. A failure leaves the app working; the
        // next launch or token tries again.
        let controller = self.clone();
        tokio::spawn(async move {
            let _ = controller.send_push().await;
        });
        Ok(())
    }

    /// Follows the workspace until stopped, retrying while offline.
    async fn run_sync(
        self: Arc<Self>,
        generation: u64,
        sync: Arc<SyncClient<HttpTransport>>,
        session: Arc<DeviceSession>,
        mut stopped: watch::Receiver<bool>,
    ) {
        let mut delay = RETRY_MIN;
        loop {
            if *stopped.borrow() {
                return;
            }
            let mut stop = stopped.clone();
            let until_stopped = async move {
                let _ = stop.wait_for(|stop| *stop).await;
            };
            let observer = self.clone();
            let observe = move |report: &SyncReport| observer.synced(generation, report);
            let result = sync
                .follow(MutationContext::new(Uuid::now_v7()), until_stopped, observe)
                .await;
            let failure = match result {
                Ok(()) => return,
                Err(SyncError::WipeRequired) => {
                    self.wipe(generation, WIPED).await;
                    return;
                }
                Err(SyncError::Unauthorized) => match session.refresh().await {
                    Ok(_) => continue,
                    Err(AuthError::WipeRequired | AuthError::InvalidGrant(_)) => {
                        self.wipe(generation, WIPED).await;
                        return;
                    }
                    Err(error) => error.to_string(),
                },
                Err(error) => error.to_string(),
            };
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            self.update(|status| status.sync_error = Some(failure));
            tokio::select! {
                _ = stopped.wait_for(|stop| *stop) => return,
                () = tokio::time::sleep(delay) => {}
            }
            delay = (delay * 2).min(RETRY_MAX);
            // Back online: the next sync also asks which records the
            // member lost access to meanwhile.
            sync.reconnected();
        }
    }

    fn synced(&self, generation: u64, _report: &SyncReport) {
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let mut open = false;
        self.update(|status| {
            status.last_synced_at = Some(Utc::now());
            status.sync_error = None;
            if status.phase == Phase::Syncing {
                status.phase = Phase::Ready;
                open = true;
            }
        });
        if open {
            let bridge = self
                .prepared
                .lock()
                .ok()
                .and_then(|prepared| prepared.clone());
            self.set_bridge(bridge);
        }
    }

    /// Rereads the member's profile at start and every [`PROFILE_EVERY`],
    /// so permissions and roles an administrator changes reach the
    /// replica's engine without another sign-in. Offline, the stored
    /// profile stays in force.
    async fn watch_profile(
        self: Arc<Self>,
        generation: u64,
        session: Arc<DeviceSession>,
        sink: Arc<Sink>,
        store: Arc<dyn ObjectStore>,
        mut stopped: watch::Receiver<bool>,
    ) {
        loop {
            if *stopped.borrow() {
                return;
            }
            if let Ok(token) = session.access().await {
                if let Ok(profile) = session.client().profile(&token, session.workspace()).await {
                    self.apply_profile(generation, &sink, &store, profile).await;
                }
            }
            tokio::select! {
                _ = stopped.wait_for(|stop| *stop) => return,
                () = tokio::time::sleep(PROFILE_EVERY) => {}
            }
        }
    }

    /// Rereads the member's profile now, as when the app returns to the
    /// foreground, and applies a change to the replica's engine.
    pub async fn refresh_profile(&self) -> Result<(), String> {
        let (generation, session, sink, files) = {
            let active = self.active.lock().await;
            let Some(active) = active.as_ref() else {
                return Ok(());
            };
            (
                active.generation,
                active.session.clone(),
                active.sink.clone(),
                active.files.clone(),
            )
        };
        let token = session.access().await.map_err(|error| error.to_string())?;
        let profile = session
            .client()
            .profile(&token, session.workspace())
            .await
            .map_err(|error| error.to_string())?;
        let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(files));
        self.apply_profile(generation, &sink, &store, profile).await;
        Ok(())
    }

    async fn apply_profile(
        &self,
        generation: u64,
        sink: &Sink,
        store: &Arc<dyn ObjectStore>,
        profile: DeviceProfile,
    ) {
        let mut stored = sink.stored.lock().await;
        if stored.profile == profile
            || sink.closed.load(Ordering::SeqCst)
            || self.generation.load(Ordering::SeqCst) != generation
        {
            return;
        }
        stored.profile = profile.clone();
        if let Err(error) = self.services.vault.save(&stored).await {
            tracing::warn!(%error, "the refreshed profile was not stored");
        }
        drop(stored);
        let bridge = bridge_for(replica_engine(store.clone(), &profile), &profile);
        if let Ok(mut prepared) = self.prepared.lock() {
            *prepared = Some(bridge.clone());
        }
        let mut open = false;
        self.update(|status| {
            status.member = Some(profile.member.display_name.clone());
            status.workspace = Some(profile.workspace.name.clone());
            open = status.phase == Phase::Ready;
        });
        if open && self.generation.load(Ordering::SeqCst) == generation {
            self.set_bridge(Some(bridge));
        }
    }

    /// Refreshes the device's tokens now, as when the app comes back to
    /// the foreground. A server that refuses the device wipes it.
    /// Takes the token the platform's push service gave the app (ADR
    /// 0030): APNs on iOS, FCM on Android. It is kept for later workspaces
    /// and sent now if one is open. The platform may hand out a new token at
    /// any time; each one replaces the last.
    pub async fn register_push(&self, provider: PushProvider, token: &str) -> Result<(), String> {
        if token.is_empty() {
            return Err("the push token is empty".into());
        }
        if let Ok(mut push) = self.push.lock() {
            *push = Some((provider, token.to_owned()));
        }
        self.send_push().await
    }

    /// Sends the platform's push token for the open workspace, if both exist.
    async fn send_push(&self) -> Result<(), String> {
        let Some((provider, token)) = self.push.lock().ok().and_then(|push| push.clone()) else {
            return Ok(());
        };
        let (generation, workspace, session) = {
            let active = self.active.lock().await;
            let Some(active) = active.as_ref() else {
                return Ok(());
            };
            (active.generation, active.workspace, active.session.clone())
        };
        let result = match session.access().await {
            Ok(access) => {
                session
                    .client()
                    .set_push(&access, workspace, provider, &token)
                    .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => Ok(()),
            Err(AuthError::WipeRequired | AuthError::InvalidGrant(_)) => {
                self.wipe(generation, WIPED).await;
                Ok(())
            }
            Err(error) => Err(message(&error)),
        }
    }

    /// One short sync for an app the platform woke in the background, after
    /// a content-free push or a scheduled refresh (ADR 0024, "Continual
    /// sync"). It never overlaps the foreground loop's steps, and stops
    /// within `budget`, the time the platform allows.
    pub async fn wake(&self, budget: Duration) -> Result<WakeOutcome, String> {
        // Registered under the lock sign-out and a wipe take the workspace
        // with, so either the wake sees no workspace or it is stopped before
        // the workspace's storage is deleted.
        let (generation, session, finished) = {
            let mut active = self.active.lock().await;
            let Some(active) = active.as_mut() else {
                return Ok(WakeOutcome::NoWorkspace);
            };
            let Some(sync) = self.sync.read().ok().and_then(|sync| sync.clone()) else {
                return Ok(WakeOutcome::NoWorkspace);
            };
            let (sender, finished) = tokio::sync::oneshot::channel();
            active.wakes.retain(|wake| !wake.is_finished());
            active.wakes.push(tokio::spawn(async move {
                let result = sync
                    .wake(MutationContext::new(Uuid::now_v7()), budget)
                    .await;
                let _ = sender.send(result);
            }));
            (active.generation, active.session.clone(), finished)
        };
        // Stopped by sign-out or a wipe: the workspace is gone.
        let Ok(result) = finished.await else {
            return Ok(WakeOutcome::NoWorkspace);
        };
        match result {
            Ok(report) => {
                self.synced(generation, &report);
                Ok(WakeOutcome::Synced)
            }
            Err(SyncError::OutOfTime) => Ok(WakeOutcome::OutOfTime),
            Err(SyncError::WipeRequired) => {
                self.wipe(generation, WIPED).await;
                Ok(WakeOutcome::Wiped)
            }
            Err(SyncError::Unauthorized) => match session.refresh().await {
                // The next wake or the foreground loop syncs with the new token.
                Ok(_) => Ok(WakeOutcome::OutOfTime),
                Err(AuthError::WipeRequired | AuthError::InvalidGrant(_)) => {
                    self.wipe(generation, WIPED).await;
                    Ok(WakeOutcome::Wiped)
                }
                Err(error) => Err(message(&error)),
            },
            Err(error) => Err(error.to_string()),
        }
    }

    pub async fn refresh_tokens(&self) -> Result<(), String> {
        let (generation, session) = {
            let active = self.active.lock().await;
            let Some(active) = active.as_ref() else {
                return Ok(());
            };
            (active.generation, active.session.clone())
        };
        match session.refresh().await {
            Ok(_) => Ok(()),
            Err(AuthError::WipeRequired | AuthError::InvalidGrant(_)) => {
                self.wipe(generation, WIPED).await;
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// Deletes the replica, its key and the stored session, once, for the
    /// workspace this generation opened.
    async fn wipe(&self, generation: u64, notice: &'static str) {
        let active = {
            let mut active = self.active.lock().await;
            match active.as_ref() {
                Some(current) if current.generation == generation => active.take(),
                _ => None,
            }
        };
        let Some(mut active) = active else {
            return;
        };
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.closing();
        stop_worker(&mut active).await;
        self.forget(&active).await;
        // Only now can another sign-in start: nothing of this workspace is
        // left to delete the new one's state.
        self.signed_out(Some(notice.to_owned()));
    }

    async fn forget(&self, active: &Active) {
        active.sink.closed.store(true, Ordering::SeqCst);
        if active.files.wipe(active.workspace).await.is_err() {
            tracing::error!("the workspace files could not be deleted");
        }
        if let Err(error) = self.services.vault.delete().await {
            tracing::error!(%error, "the stored session could not be deleted");
        }
    }

    /// Signs the device out: tells the server, then deletes the replica,
    /// its key and the stored session, even when the server cannot be
    /// reached (the member can still revoke the device on the web).
    pub async fn sign_out(&self) {
        self.cancel_sign_in();
        let active = self.active.lock().await.take();
        let Some(mut active) = active else {
            if self.phase() == Phase::Demo {
                self.signed_out(None);
            }
            return;
        };
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.closing();
        stop_worker(&mut active).await;
        let reached = tokio::time::timeout(SIGN_OUT_WAIT, async {
            let token = active.session.access().await.ok()?;
            active
                .session
                .client()
                .sign_out(&token, active.workspace)
                .await
                .ok()
        })
        .await
        .ok()
        .flatten()
        .is_some();
        self.forget(&active).await;
        self.signed_out(Some(if reached {
            "Signed out. This device's copy of the workspace was deleted.".into()
        } else {
            "Signed out. This device's copy of the workspace was deleted; the server could not be reached, so revoke the device in Settings → Devices on the web.".into()
        }));
    }

    /// Opens the demonstration workspace the shell built.
    pub fn show_demo(&self, bridge: Bridge) -> Result<(), SignInError> {
        let mut status = self.status.lock().map_err(|_| SignInError::Busy)?;
        if status.phase != Phase::SignedOut {
            return Err(SignInError::Busy);
        }
        *status = Status {
            phase: Phase::Demo,
            workspace: Some("Demonstration".into()),
            ..Status::default()
        };
        drop(status);
        self.set_bridge(Some(bridge));
        Ok(())
    }
}

/// The replica's engine, acting as the member the profile describes.
fn replica_engine(store: Arc<dyn ObjectStore>, profile: &DeviceProfile) -> ObjectService {
    ObjectService::new(
        store,
        ObjectCaller::new(
            profile.workspace.id,
            profile.member.id,
            profile.member.permissions.clone(),
            profile.member.role_ids.clone(),
        ),
    )
    .with_replica()
    .with_client_ids()
}

fn bridge_for(engine: ObjectService, profile: &DeviceProfile) -> Bridge {
    Bridge::new(
        DeviceHost::new(
            engine,
            Member {
                display_name: profile.member.display_name.clone(),
                email: profile.member.email.clone(),
                permissions: profile.member.permissions.clone(),
                roles: profile.member.roles.clone(),
            },
            Workspace {
                name: profile.workspace.name.clone(),
            },
        )
        .with_device(profile.device_id),
    )
}

/// Stops the workspace's sync worker before its storage goes: it finishes
/// a step in progress for up to [`SIGN_OUT_WAIT`], then is aborted, so it
/// cannot write the replica or store a token after the wipe. The worker
/// itself, wiping on the server's word, does not wait for itself.
async fn stop_worker(active: &mut Active) {
    let _ = active.stop.send(true);
    if let Some(profile_task) = active.profile_task.take() {
        // It may be mid-request; nothing it holds outlives the wipe.
        profile_task.abort();
        let _ = profile_task.await;
    }
    // A wake in flight finishes its step, or is cut off, before the storage
    // it writes to is deleted.
    for mut wake in active.wakes.drain(..) {
        if tokio::time::timeout(SIGN_OUT_WAIT, &mut wake)
            .await
            .is_err()
        {
            wake.abort();
            let _ = wake.await;
        }
    }
    let Some(mut task) = active.task.take() else {
        return;
    };
    if tokio::task::try_id() == Some(task.id()) {
        return;
    }
    if tokio::time::timeout(SIGN_OUT_WAIT, &mut task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_address_names_its_server_and_workspace() {
        let workspace = Uuid::now_v7();
        assert_eq!(
            parse_address(&format!(
                " https://limiara.example.com/workspaces/{workspace} "
            )),
            Ok(("https://limiara.example.com".into(), workspace))
        );
        assert_eq!(
            parse_address(&format!("http://localhost:8080/{workspace}?x=1")),
            Ok(("http://localhost:8080".into(), workspace))
        );
        assert_eq!(
            parse_address("https://limiara.example.com/workspaces/"),
            Err(SignInError::InvalidAddress)
        );
        assert_eq!(
            parse_address(&format!("limiara.example.com/{workspace}")),
            Err(SignInError::InvalidAddress)
        );
    }

    #[test]
    fn loopback_redirects_stay_on_loopback() {
        assert!(LoopbackRedirects::new("http://127.0.0.1:47185/callback").is_ok());
        assert!(LoopbackRedirects::new("http://[::1]:47185/callback").is_ok());
        assert!(LoopbackRedirects::new("http://localhost:47185/callback").is_err());
        assert!(LoopbackRedirects::new("http://192.168.1.2:47185/callback").is_err());
        assert!(LoopbackRedirects::new("https://127.0.0.1:47185/callback").is_err());
        assert!(LoopbackRedirects::new("http://127.0.0.1/callback").is_err());
    }

    #[tokio::test]
    async fn the_loopback_listener_returns_the_callback_only() {
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe");
            probe.local_addr().expect("address").port()
        };
        let redirects = LoopbackRedirects::new(&format!("http://127.0.0.1:{port}/callback"))
            .expect("redirects");
        let pending = redirects.prepare().await.expect("listening");
        let send = move |target: &'static str| async move {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            stream
                .write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .await
                .expect("write");
            let mut response = String::new();
            stream.read_to_string(&mut response).await.expect("read");
            response
        };
        let client = tokio::spawn(async move {
            let favicon = send("/favicon.ico").await;
            let callback = send("/callback?code=abc&state=xyz").await;
            (favicon, callback)
        });
        let url = pending.await.expect("redirect");
        let (favicon, callback) = client.await.expect("client");
        assert!(favicon.starts_with("HTTP/1.1 404"));
        assert!(callback.starts_with("HTTP/1.1 200"));
        assert!(callback.contains("return to the Limiara app"));
        assert_eq!(url.path(), "/callback");
        assert_eq!(url.query(), Some("code=abc&state=xyz"));
    }
}
