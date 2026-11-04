//! Drives the app's [`Controller`] headless against a real server, for
//! `scripts/test-device-app-sign-in.sh` (#385). It prints `OPEN <url>`
//! where an app would open the system browser, and the driver signs the
//! person in there. Then it checks, in order:
//!
//! 1. the sign-in completes, the first sync opens the replica, the bridge
//!    answers as the member, and the offline limit reaches the replica;
//! 2. a profile changed on the server reaches the open workspace;
//! 3. after the driver revokes the device (`revoked` on stdin), the next
//!    refresh wipes the replica, its key and the stored session;
//! 4. a second sign-in, then sign-out, deletes the replica again.
//!
//! Environment: `LIMIARA_ADDRESS` (the workspace address), `REDIRECT_URI`
//! (a registered loopback redirect) and `DATA_DIR`.

use platform_device::app::{
    AppSettings, Browser, Controller, LoopbackRedirects, MemoryVault, Phase, Services,
    SessionVault, Status,
};
use platform_device::{device_databases, MemoryKeys};
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

struct Printed;

impl Browser for Printed {
    fn open(&self, url: &str) -> Result<(), String> {
        println!("OPEN {url}");
        std::io::stdout().flush().map_err(|error| error.to_string())
    }
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

async fn until(controller: &Controller, phase: Phase) -> Status {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let status = controller.status().await;
        if status.phase == phase {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "waited for {phase:?}: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn bridge_get(controller: &Controller, path: &str) -> (u16, serde_json::Value) {
    let bridge = controller.bridge().expect("an open workspace");
    let request = http::Request::get(path).body(Vec::new()).expect("request");
    let response = bridge.handle(request).await;
    let body = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
    (response.status().as_u16(), body)
}

#[tokio::main]
async fn main() {
    let address = env("LIMIARA_ADDRESS");
    let (_, workspace) = platform_device::app::parse_address(&address).expect("address");
    let directory = std::path::PathBuf::from(env("DATA_DIR"));
    let vault = Arc::new(MemoryVault::default());
    let keys = Arc::new(MemoryKeys::new());
    let redirect_uri = env("REDIRECT_URI");
    let controller = Controller::new(
        &directory,
        AppSettings {
            redirect_uri: redirect_uri.clone(),
            device_name: "Probe laptop".into(),
            platform: "desktop".into(),
        },
        Services {
            vault: vault.clone(),
            keys: keys.clone(),
            browser: Arc::new(Printed),
            redirects: Arc::new(LoopbackRedirects::new(&redirect_uri).expect("loopback")),
        },
    );
    let file = device_databases(&directory, keys.clone()).path_for(workspace);

    println!("== a member signs in and the first sync opens the replica");
    controller.start_sign_in(&address).expect("sign-in starts");
    let ready = until(&controller, Phase::Ready).await;
    assert_eq!(ready.member.as_deref(), Some("Morgan Member"), "{ready:?}");
    assert_eq!(ready.workspace.as_deref(), Some("Local workspace"));
    assert!(ready.last_synced_at.is_some(), "the first sync ran");
    let lease = ready.lease.expect("a lease");
    let expires = lease
        .expires_at
        .expect("the offline limit reached the replica");
    let days = (expires - chrono::Utc::now()).num_days();
    assert!((13..=14).contains(&days), "the 14-day limit: {expires}");
    assert!(!lease.locked);
    let (status, me) = bridge_get(&controller, "/api/v1/me").await;
    assert_eq!(status, 200);
    assert_eq!(me["user"]["display_name"], "Morgan Member");
    let (status, _) = bridge_get(
        &controller,
        &format!("/api/v1/workspaces/{workspace}/objects"),
    )
    .await;
    assert_eq!(status, 200, "the bridge serves the synced workspace");
    assert!(file.exists(), "the replica is on disk");
    let stored = vault
        .load()
        .await
        .expect("vault")
        .expect("a stored session");
    assert_eq!(stored.profile.workspace.id, workspace);

    println!("WAITING rename");
    std::io::stdout().flush().expect("flush");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).expect("stdin");
    assert_eq!(line.trim(), "renamed");

    println!("== a changed profile reaches the open workspace");
    controller.refresh_profile().await.expect("profile");
    assert_eq!(
        controller.status().await.member.as_deref(),
        Some("Morgan M. Member")
    );
    let (_, me) = bridge_get(&controller, "/api/v1/me").await;
    assert_eq!(me["user"]["display_name"], "Morgan M. Member");
    let stored = vault.load().await.expect("vault").expect("a stored session");
    assert_eq!(stored.profile.member.display_name, "Morgan M. Member");
    println!("REFRESHED");

    println!("WAITING revoke");
    std::io::stdout().flush().expect("flush");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).expect("stdin");
    assert_eq!(line.trim(), "revoked");

    println!("== the next refresh of a revoked device wipes it");
    controller.refresh_tokens().await.expect("refresh");
    let wiped = until(&controller, Phase::SignedOut).await;
    assert!(
        wiped
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("deleted")),
        "{wiped:?}"
    );
    assert!(controller.bridge().is_none());
    assert!(!file.exists(), "the replica was deleted");
    assert!(!keys.holds(workspace).await, "its key was destroyed");
    assert!(
        vault.load().await.expect("vault").is_none(),
        "the session was forgotten"
    );
    println!("WIPED");

    println!("== a member signs in again and signs out");
    controller.start_sign_in(&address).expect("sign-in starts");
    until(&controller, Phase::Ready).await;
    assert!(file.exists());
    controller.sign_out().await;
    let signed_out = controller.status().await;
    assert_eq!(signed_out.phase, Phase::SignedOut);
    assert_eq!(
        signed_out.notice.as_deref(),
        Some("Signed out. This device's copy of the workspace was deleted.")
    );
    assert!(!file.exists(), "sign-out deleted the replica");
    assert!(vault.load().await.expect("vault").is_none());
    println!("SIGNED_OUT");
}
