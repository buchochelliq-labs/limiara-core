//! The desktop keychain adapter against the platform's real keychain (the
//! kernel keyring on Linux), under a service name of the test's own.

use device_shell::keychain::Keychain;
use platform_device::app::{
    DeviceProfile, ProfileMember, ProfileWorkspace, SessionVault, StoredSession,
};
use platform_device::DatabaseKeys;
use std::collections::BTreeSet;
use uuid::Uuid;

fn keychain() -> Keychain {
    Keychain::new(format!("app.limiara.shell.test-{}", Uuid::now_v7()))
}

#[tokio::test]
async fn the_session_is_kept_replaced_and_forgotten() {
    let keychain = keychain();
    assert!(keychain.load().await.expect("load").is_none());
    let mut session = StoredSession {
        server: "https://limiara.example.com".into(),
        refresh_token: "refresh-1".into(),
        offline_limit_seconds: 1_209_600,
        profile: DeviceProfile {
            device_id: Uuid::now_v7(),
            member: ProfileMember {
                id: Uuid::now_v7(),
                display_name: "Ada".into(),
                email: None,
                permissions: BTreeSet::new(),
                roles: BTreeSet::from(["Engineer".to_owned()]),
                role_ids: vec![Uuid::now_v7()],
            },
            workspace: ProfileWorkspace {
                id: Uuid::now_v7(),
                name: "Engines".into(),
            },
        },
    };
    keychain.save(&session).await.expect("save");
    session.refresh_token = "refresh-2".into();
    keychain.save(&session).await.expect("rotate");
    let loaded = keychain.load().await.expect("load").expect("a session");
    assert_eq!(loaded.refresh_token, "refresh-2");
    assert_eq!(loaded.profile, session.profile);
    keychain.delete().await.expect("delete");
    assert!(keychain.load().await.expect("load").is_none());
}

#[tokio::test]
async fn a_workspace_key_is_created_once_and_destroyed() {
    let keychain = keychain();
    let workspace = Uuid::now_v7();
    let first = keychain.key(workspace).await.expect("key");
    let again = keychain.key(workspace).await.expect("key");
    assert_eq!(first.as_bytes(), again.as_bytes());
    let other = Uuid::now_v7();
    assert_ne!(
        keychain.key(other).await.expect("other").as_bytes(),
        first.as_bytes()
    );
    keychain.destroy(other).await.expect("cleanup");
    keychain.destroy(workspace).await.expect("destroy");
    let fresh = keychain.key(workspace).await.expect("fresh key");
    assert_ne!(fresh.as_bytes(), first.as_bytes());
    keychain.destroy(workspace).await.expect("cleanup");
}
