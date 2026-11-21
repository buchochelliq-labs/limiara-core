//! Optional dependencies on every backend (ADR 0035 G18): a plugin may name
//! objects of a plugin it optionally depends on in a related-to field's
//! allowlist. Each such entry waits until that object is active in the
//! workspace, joins the allowlist once, offers the plugin's related actions
//! as any new entry does, and is never added back after an administrator
//! removes it, through re-enabling or reinstalling either plugin. Either
//! install order ends the same way.

use super::{context, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const ACTIVITIES: &str = "platform.activities";
const SALES: &str = "platform.sales";
const TASK: &str = "platform.activities:task";
const EVENT: &str = "platform.activities:event";
const RELATED_TO: &str = "platform.activities:related_to";
const WHO: &str = "platform.activities:who";
const LEAD: &str = "platform.sales:lead";
const OPPORTUNITY: &str = "platform.sales:opportunity";

async fn allowlist(admin: &ObjectService, object: &str, field: &str) -> Vec<String> {
    admin
        .describe(object)
        .await
        .expect("describe")
        .0
        .field(field)
        .expect("field")
        .reference_objects
        .clone()
}

async fn action_names(admin: &ObjectService, object: &str) -> Vec<String> {
    admin
        .quick_actions(object)
        .await
        .expect("quick actions")
        .into_iter()
        .map(|action| action.name)
        .collect()
}

async fn enable(store: &Arc<dyn ObjectStore>, tenant: Uuid, member: Uuid, plugin: &str) {
    let installer = service(
        store,
        ObjectCaller::plugin_installer(tenant, member, plugin),
    );
    let manifest = if plugin == SALES {
        super::sales::sales()
    } else {
        super::related_actions::activities()
    };
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    installer
        .resume_plugin_objects(plugin, context())
        .await
        .expect("resume");
}

async fn disable(store: &Arc<dyn ObjectStore>, tenant: Uuid, member: Uuid, plugin: &str) {
    service(
        store,
        ObjectCaller::plugin_installer(tenant, member, plugin),
    )
    .suspend_plugin_objects(plugin, context())
    .await
    .expect("suspend");
}

async fn assert_joined(admin: &ObjectService) {
    for object in [TASK, EVENT] {
        assert_eq!(allowlist(admin, object, RELATED_TO).await, [OPPORTUNITY]);
        assert_eq!(allowlist(admin, object, WHO).await, [LEAD]);
    }
    // Activities offers Log a call on its Related to targets only.
    assert_eq!(action_names(admin, OPPORTUNITY).await, ["log_a_call"]);
    assert!(action_names(admin, LEAD).await.is_empty());
}

/// Activities first, then Sales; `tenant_b` installs them the other way.
pub async fn run(store: Arc<dyn ObjectStore>, tenant_a: Uuid, tenant_b: Uuid) {
    let member = Uuid::now_v7();
    let admin = service(
        &store,
        super::caller(tenant_a, member, &ADMIN_PERMISSIONS, &[]),
    );

    // --- Without Sales, the entries wait and the allowlists stay empty ------
    enable(&store, tenant_a, member, ACTIVITIES).await;
    for object in [TASK, EVENT] {
        assert!(allowlist(&admin, object, RELATED_TO).await.is_empty());
        assert!(allowlist(&admin, object, WHO).await.is_empty());
    }

    // --- Installing Sales alone does not offer: its objects are not active --
    let sales_installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant_a, member, SALES),
    );
    sales_installer
        .install_plugin_objects(&super::sales::sales(), context())
        .await
        .expect("install Sales");
    assert!(allowlist(&admin, TASK, RELATED_TO).await.is_empty());

    // --- Enabling Sales: its objects join, with Log a call ------------------
    sales_installer
        .resume_plugin_objects(SALES, context())
        .await
        .expect("enable Sales");
    assert_joined(&admin).await;

    // --- An administrator's removal sticks ----------------------------------
    let version = admin
        .describe(TASK)
        .await
        .expect("task")
        .0
        .field(RELATED_TO)
        .expect("related to")
        .version;
    admin
        .update_field(
            TASK,
            RELATED_TO,
            serde_json::from_value(json!({"expected_version": version, "reference_objects": []}))
                .expect("change"),
            context(),
        )
        .await
        .expect("an administrator removes the opportunity");
    for plugin in [SALES, ACTIVITIES] {
        disable(&store, tenant_a, member, plugin).await;
        enable(&store, tenant_a, member, plugin).await;
    }
    assert!(allowlist(&admin, TASK, RELATED_TO).await.is_empty());
    assert_eq!(allowlist(&admin, EVENT, RELATED_TO).await, [OPPORTUNITY]);
    assert_eq!(allowlist(&admin, TASK, WHO).await, [LEAD]);

    // --- The other order: Sales first, then Activities ----------------------
    let admin_b = service(
        &store,
        super::caller(tenant_b, member, &ADMIN_PERMISSIONS, &[]),
    );
    enable(&store, tenant_b, member, SALES).await;
    enable(&store, tenant_b, member, ACTIVITIES).await;
    assert_joined(&admin_b).await;
}
