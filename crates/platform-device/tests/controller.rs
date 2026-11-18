//! The controller's side of the replica change feed and the route context
//! (ADR 0034 §9): opening a workspace rebuilds subscribers, sign-out and a
//! wipe end the workspace with `workspace_cleared` before its storage goes,
//! and nothing of it follows. Plugins reach both only through the grant.

use axum::routing::post;
use platform_device::app::{
    AppSettings, Browser, Controller, DeviceProfile, LoopbackRedirects, MemoryVault, ProfileMember,
    ProfileWorkspace, Services, SessionVault, StoredSession,
};
use platform_device::capabilities::PluginDeclaration;
use platform_device::feed::{ChangeSubscription, ClearReason, RebuildReason, ReplicaChange};
use platform_device::route::RoutePattern;
use platform_device::MemoryKeys;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

struct NoBrowser;

impl Browser for NoBrowser {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err("no browser in tests".into())
    }
}

fn stored(server: String, workspace: Uuid) -> StoredSession {
    StoredSession {
        server,
        refresh_token: "refresh".into(),
        offline_limit_seconds: 3_600,
        profile: DeviceProfile {
            device_id: Uuid::now_v7(),
            member: ProfileMember {
                id: Uuid::now_v7(),
                display_name: "Ada Lovelace".into(),
                email: None,
                permissions: BTreeSet::new(),
                roles: BTreeSet::new(),
                role_ids: Vec::new(),
            },
            workspace: ProfileWorkspace {
                id: workspace,
                name: "Analytical Engines".into(),
            },
        },
        assistant: Default::default(),
    }
}

async fn controller(
    directory: &std::path::Path,
    session: StoredSession,
) -> (Arc<Controller>, Arc<MemoryVault>) {
    controller_on(directory, session, "desktop").await
}

async fn controller_on(
    directory: &std::path::Path,
    session: StoredSession,
    platform: &str,
) -> (Arc<Controller>, Arc<MemoryVault>) {
    let vault = Arc::new(MemoryVault::default());
    vault.save(&session).await.expect("saved");
    let controller = Controller::new(
        directory,
        AppSettings {
            redirect_uri: "http://127.0.0.1:47185/callback".into(),
            device_name: "Test".into(),
            platform: platform.into(),
        },
        Services {
            vault: vault.clone(),
            keys: Arc::new(MemoryKeys::new()),
            browser: Arc::new(NoBrowser),
            redirects: Arc::new(
                LoopbackRedirects::new("http://127.0.0.1:47185/callback").expect("loopback"),
            ),
        },
    );
    (controller, vault)
}

fn subscribe(controller: &Controller) -> ChangeSubscription {
    controller
        .grant(
            &PluginDeclaration::new("feed-test", &["device.replica_changes"]).expect("declaration"),
        )
        .replica_changes()
        .expect("declared")
}

/// The events until `last`, or until none comes for a while.
async fn until(
    subscription: &mut ChangeSubscription,
    last: &ReplicaChange,
) -> Vec<(Option<Uuid>, ReplicaChange)> {
    let mut seen = Vec::new();
    while let Some(event) = subscription.next_within(Duration::from_secs(20)).await {
        let done = &event.change == last;
        seen.push((event.workspace, event.change));
        if done {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn sign_out_ends_the_workspace_with_workspace_cleared() {
    let directory = tempfile::tempdir().expect("dir");
    let workspace = Uuid::now_v7();
    // A server that cannot be reached: the replica opens offline, and
    // sign-out goes ahead without it.
    let (controller, vault) = controller(
        directory.path(),
        stored("http://127.0.0.1:1".into(), workspace),
    )
    .await;
    let mut feed = subscribe(&controller);
    let routes = controller
        .grant(&PluginDeclaration::new("route-test", &["device.route_context"]).expect("declared"));
    let routes = routes.route_context().expect("declared");

    controller.resume().await;
    let bridge = controller.bridge().expect("the replica opens offline");
    let response = bridge
        .handle(
            http::Request::builder()
                .method("POST")
                .uri(format!("/api/v1/workspaces/{workspace}/ui/route"))
                .header("content-type", "application/json")
                .body(br#"{"route":"home"}"#.to_vec())
                .expect("request"),
        )
        .await;
    assert_eq!(response.status(), 204);
    assert_eq!(
        routes.current().map(|route| (route.workspace, route.route)),
        Some((workspace, RoutePattern::Home))
    );

    controller.sign_out().await;
    let cleared = ReplicaChange::WorkspaceCleared {
        reason: ClearReason::SignedOut,
    };
    let events = until(&mut feed, &cleared).await;
    assert_eq!(
        events.first(),
        Some(&(
            None,
            ReplicaChange::Rebuild {
                reason: RebuildReason::Subscribed
            }
        )),
        "{events:?}"
    );
    assert_eq!(
        events.get(1),
        Some(&(
            Some(workspace),
            ReplicaChange::Rebuild {
                reason: RebuildReason::Opened
            }
        )),
        "{events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&(Some(workspace), cleared)),
        "{events:?}"
    );
    // It is the last event for the workspace.
    assert!(feed.next_within(Duration::from_millis(500)).await.is_none());
    assert_eq!(controller.local_services().feed().workspace(), None);
    // The route went with it, and so did the stored session.
    assert_eq!(routes.current(), None);
    assert!(vault.load().await.expect("vault").is_none());
}

#[tokio::test]
async fn a_wipe_ends_the_workspace_with_workspace_cleared() {
    // A server that refuses the device and asks for its copy to be wiped.
    let app = axum::Router::new().route(
        "/oauth/token",
        post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": "invalid_grant", "wipe": true})),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("port");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, app).await });

    let directory = tempfile::tempdir().expect("dir");
    let workspace = Uuid::now_v7();
    let (controller, vault) = controller(
        directory.path(),
        stored(format!("http://{address}"), workspace),
    )
    .await;
    let mut feed = subscribe(&controller);
    // The sync loop's first request meets the refusal and wipes.
    controller.resume().await;
    let cleared = ReplicaChange::WorkspaceCleared {
        reason: ClearReason::Wiped,
    };
    let events = until(&mut feed, &cleared).await;
    assert_eq!(
        events.last(),
        Some(&(Some(workspace), cleared)),
        "{events:?}"
    );
    assert!(feed.next_within(Duration::from_millis(500)).await.is_none());
    assert!(controller.bridge().is_none());
    assert!(vault.load().await.expect("vault").is_none());
}

#[tokio::test]
async fn plugins_get_only_the_capabilities_they_declared() {
    let directory = tempfile::tempdir().expect("dir");
    let (controller, _) = controller(
        directory.path(),
        stored("http://127.0.0.1:1".into(), Uuid::now_v7()),
    )
    .await;
    let probe = controller
        .grant(&PluginDeclaration::new("probe", &["device.route_context"]).expect("declared"));
    assert_eq!(
        probe
            .replica_changes()
            .map(|_| ())
            .map_err(|error| error.code()),
        Err("device.capability_not_granted")
    );
    assert!(probe.route_context().is_ok());
    assert_eq!(
        probe.assistant().map(|_| ()).map_err(|error| error.code()),
        Err("device.capability_not_granted")
    );
    assert_eq!(
        PluginDeclaration::new("probe", &["device.camera"]).map_err(|error| error.code()),
        Err("device.capability_unknown")
    );
}

async fn ask(
    bridge: &platform_device::assistant::AssistantBridge,
    assistant: platform_objects::Assistant,
) -> Option<platform_device::assistant::errors::AssistantCode> {
    use platform_device::assistant::{AssistantRequest, Operation};
    bridge
        .call(AssistantRequest::new(assistant, true, Operation::Describe))
        .await
        .into_result()
        .err()
        .map(|error| error.code)
}

#[tokio::test]
async fn the_assistant_bridge_follows_the_signed_in_workspace() {
    use platform_device::assistant::errors::AssistantCode;
    use platform_device::assistant::DeviceAssistantSettings;
    use platform_objects::Assistant;
    let directory = tempfile::tempdir().expect("dir");
    let (phone, vault) = controller_on(
        directory.path(),
        stored("http://127.0.0.1:1".into(), Uuid::now_v7()),
        "ios",
    )
    .await;
    let bridge = phone
        .grant(&PluginDeclaration::new("siri", &["device.assistant"]).expect("declared"))
        .assistant()
        .expect("granted");
    // Before the workspace opens, and for the wrong assistant.
    assert_eq!(
        ask(&bridge, Assistant::Siri).await,
        Some(AssistantCode::SignedOut)
    );
    phone.resume().await;
    assert_eq!(
        ask(&bridge, Assistant::Gemini).await,
        Some(AssistantCode::Invalid)
    );
    // Open, with no policy taken yet: assistant access is off.
    assert_eq!(
        ask(&bridge, Assistant::Siri).await,
        Some(AssistantCode::Disabled)
    );
    // The member's device setting is kept with the session.
    assert_eq!(
        phone.assistant_settings(),
        DeviceAssistantSettings::default()
    );
    let off = DeviceAssistantSettings {
        assistant_access: false,
        indexing: true,
    };
    phone.set_assistant_settings(off).await.expect("saved");
    assert_eq!(
        vault
            .load()
            .await
            .expect("vault")
            .map(|stored| stored.assistant),
        Some(off)
    );
    assert_eq!(phone.assistant_settings(), off);
    // Sign-out closes it before the replica goes.
    phone.sign_out().await;
    assert_eq!(
        ask(&bridge, Assistant::Siri).await,
        Some(AssistantCode::SignedOut)
    );

    // The desktop has no assistant.
    let directory = tempfile::tempdir().expect("dir");
    let (desktop, _) = controller(
        directory.path(),
        stored("http://127.0.0.1:1".into(), Uuid::now_v7()),
    )
    .await;
    desktop.resume().await;
    let bridge = desktop
        .grant(&PluginDeclaration::new("any", &["device.assistant"]).expect("declared"))
        .assistant()
        .expect("granted");
    assert_eq!(
        ask(&bridge, Assistant::Siri).await,
        Some(AssistantCode::Invalid)
    );
    desktop.sign_out().await;
}
