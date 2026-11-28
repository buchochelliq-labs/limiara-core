//! The `limiara://` scheme handler, as the webview sees it: the embedded web
//! build, the shell's routes, the app's sign-in route, and the API answered
//! by the bridge over the demonstration workspace. The requests are those the web shell makes on
//! start (`apps/web/src/shell/data.ts`) and on an object's pages
//! (`apps/web/src/objects/api.ts`).

use device_shell::{fixture, Asset, Shell, CONTENT_SECURITY_POLICY};
use http::{Request, Response};
use platform_device::app::{
    AppSettings, Browser, Controller, LoopbackRedirects, MemoryVault, Services,
};
use platform_device::MemoryKeys;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

fn asset(body: &str, mime_type: &str) -> Asset {
    Asset {
        bytes: body.as_bytes().to_vec(),
        mime_type: mime_type.into(),
    }
}

struct NoBrowser;

impl Browser for NoBrowser {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err("no browser in tests".into())
    }
}

/// A signed-out shell.
fn signed_out_shell(directory: &std::path::Path) -> Shell {
    let assets = HashMap::from([
        (
            "/index.html".to_owned(),
            asset("<div id=\"root\"></div>", "text/html"),
        ),
        (
            "/assets/index.js".to_owned(),
            asset("console.log(1)", "text/javascript"),
        ),
    ]);
    let controller = Controller::new(
        directory.join("workspaces"),
        AppSettings {
            redirect_uri: "http://127.0.0.1:47185/callback".into(),
            device_name: "Test".into(),
            platform: "desktop".into(),
        },
        Services {
            vault: Arc::new(MemoryVault::default()),
            keys: Arc::new(MemoryKeys::new()),
            browser: Arc::new(NoBrowser),
            redirects: Arc::new(
                LoopbackRedirects::new("http://127.0.0.1:47185/callback").expect("loopback"),
            ),
        },
    );
    Shell::new(
        controller,
        Arc::new(assets),
        directory.join("demonstration"),
    )
}

/// A shell with the demonstration workspace open.
async fn open_shell(directory: &std::path::Path) -> Shell {
    let shell = signed_out_shell(directory);
    let host = fixture::open(&directory.join("demonstration"))
        .await
        .expect("demo workspace");
    shell.controller().show_demo(host).expect("demo");
    shell
}

async fn send(shell: &Shell, method: &str, path: &str, body: Option<Value>) -> Response<Vec<u8>> {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("limiara://localhost{path}"));
    let bytes = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            serde_json::to_vec(&body).expect("body")
        }
        None => Vec::new(),
    };
    shell.respond(request.body(bytes).expect("request")).await
}

fn json_body(response: &Response<Vec<u8>>) -> Value {
    serde_json::from_slice(response.body()).expect("JSON body")
}

#[tokio::test]
async fn the_scheme_serves_the_shell_and_its_routes() {
    let directory = tempfile::tempdir().expect("dir");
    let shell = open_shell(directory.path()).await;

    let index = send(&shell, "GET", "/", None).await;
    assert_eq!(index.status(), 200);
    assert_eq!(index.headers()["content-type"], "text/html");
    assert_eq!(
        index.headers()["content-security-policy"],
        CONTENT_SECURITY_POLICY
    );
    assert_eq!(index.headers()["x-content-type-options"], "nosniff");

    // The shell's own routes load the shell.
    let route = send(&shell, "GET", "/objects/custom:account", None).await;
    assert_eq!(route.status(), 200);
    assert_eq!(route.body(), index.body());
    // A plugin object's name holds a dot, which is not a file extension.
    for path in [
        "/o/platform.sales:opportunity",
        "/o/platform.sales%3Aopportunity",
    ] {
        let route = send(&shell, "GET", path, None).await;
        assert_eq!(route.status(), 200, "{path}");
        assert_eq!(route.body(), index.body(), "{path}");
    }

    let script = send(&shell, "GET", "/assets/index.js", None).await;
    assert_eq!(script.status(), 200);
    assert_eq!(script.headers()["content-type"], "text/javascript");

    // A missing file is not the shell, and nothing climbs out of the build.
    assert_eq!(
        send(&shell, "GET", "/assets/missing.js", None)
            .await
            .status(),
        404
    );
    assert_eq!(
        send(&shell, "GET", "/assets/../index.html", None)
            .await
            .status(),
        404
    );
    assert_eq!(
        send(&shell, "POST", "/index.html", None).await.status(),
        405
    );
}

#[tokio::test]
async fn the_api_is_the_bridge_over_the_demonstration_workspace() {
    let directory = tempfile::tempdir().expect("dir");
    let shell = open_shell(directory.path()).await;
    let workspace = fixture::WORKSPACE;

    let me = send(&shell, "GET", "/api/v1/me", None).await;
    assert_eq!(me.status(), 200);
    assert_eq!(me.headers()["cache-control"], "no-store");
    assert_eq!(json_body(&me)["user"]["display_name"], "Demo member");

    let workspaces = json_body(&send(&shell, "GET", "/api/v1/workspaces", None).await);
    assert_eq!(workspaces["data"][0]["id"], json!(workspace));

    let ui = json_body(
        &send(
            &shell,
            "GET",
            &format!("/api/v1/workspaces/{workspace}/ui/contributions"),
            None,
        )
        .await,
    );
    assert!(
        ui["navigation"]
            .as_array()
            .expect("navigation")
            .iter()
            .any(|item| item["id"] == "objects.custom:account"),
        "{ui}"
    );

    let records = format!("/api/v1/workspaces/{workspace}/objects/custom:account/records");
    let page = json_body(&send(&shell, "GET", &records, None).await);
    assert_eq!(page["data"].as_array().map(Vec::len), Some(4), "{page}");

    let created = send(
        &shell,
        "POST",
        &records,
        Some(json!({"name": "Babbage & Co", "custom:city": "London"})),
    )
    .await;
    assert_eq!(created.status(), 201, "{}", json_body(&created));
    let id = json_body(&created)["id"].as_str().expect("id").to_owned();
    let version = json_body(&created)["version"].as_i64().expect("version");
    let updated = send(
        &shell,
        "PATCH",
        &format!("{records}/{id}"),
        Some(json!({"expected_version": version, "custom:notes": "first visit"})),
    )
    .await;
    assert_eq!(updated.status(), 200, "{}", json_body(&updated));
    assert_eq!(json_body(&updated)["custom:notes"], "first visit");

    // A launch starts the demonstration afresh.
    let relaunched = open_shell(directory.path()).await;
    let page = json_body(&send(&relaunched, "GET", &records, None).await);
    assert_eq!(page["data"].as_array().map(Vec::len), Some(4), "{page}");
}

#[tokio::test]
async fn a_signed_out_shell_offers_sign_in_and_the_demonstration() {
    let directory = tempfile::tempdir().expect("dir");
    let shell = signed_out_shell(directory.path());

    let session = send(&shell, "GET", "/api/device/session", None).await;
    assert_eq!(session.status(), 200);
    assert_eq!(session.headers()["cache-control"], "no-store");
    assert!(session.headers().contains_key("content-security-policy"));
    assert_eq!(json_body(&session)["phase"], "signed_out");

    // No workspace, so the web shell's first request is refused.
    let me = send(&shell, "GET", "/api/v1/me", None).await;
    assert_eq!(me.status(), 401);
    assert_eq!(json_body(&me)["code"], "auth.signed_out");

    let invalid = send(
        &shell,
        "POST",
        "/api/device/session",
        Some(json!({"address": "not a workspace"})),
    )
    .await;
    assert_eq!(invalid.status(), 400);
    assert_eq!(json_body(&invalid)["code"], "device.address_invalid");

    // A browser that does not open ends the sign-in with a notice.
    let started = send(
        &shell,
        "POST",
        "/api/device/session",
        Some(json!({"address": format!("https://limiara.example.com/workspaces/{}", uuid::Uuid::now_v7())})),
    )
    .await;
    assert_eq!(started.status(), 200);
    let mut status = json_body(&started);
    for _ in 0..50 {
        if status["phase"] == "signed_out" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        status = json_body(&send(&shell, "GET", "/api/device/session", None).await);
    }
    assert_eq!(status["phase"], "signed_out", "{status}");
    assert!(
        status["notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("browser")),
        "{status}"
    );

    let demo = send(
        &shell,
        "POST",
        "/api/device/session",
        Some(json!({"demo": true})),
    )
    .await;
    assert_eq!(json_body(&demo)["phase"], "demo");
    assert_eq!(send(&shell, "GET", "/api/v1/me", None).await.status(), 200);
    let busy = send(
        &shell,
        "POST",
        "/api/device/session",
        Some(json!({"demo": true})),
    )
    .await;
    assert_eq!(busy.status(), 409);

    let left = send(&shell, "DELETE", "/api/device/session", None).await;
    assert_eq!(json_body(&left)["phase"], "signed_out");
    assert_eq!(send(&shell, "GET", "/api/v1/me", None).await.status(), 401);
    assert_eq!(
        send(&shell, "PUT", "/api/device/session", None)
            .await
            .status(),
        405
    );
}

/// The demonstration shows what a workspace holds, through the routes the
/// web shell calls: the Sales and Activities plugins' objects, workspace
/// objects with many field types, formulas, related lists, the Activity
/// timeline, quick actions and a validation rule.
#[tokio::test]
async fn the_demonstration_shows_the_platform() {
    let directory = tempfile::tempdir().expect("dir");
    let shell = open_shell(directory.path()).await;
    let workspace = fixture::WORKSPACE;
    let api = format!("/api/v1/workspaces/{workspace}");
    let get = |path: String| {
        let shell = &shell;
        async move {
            let response = send(shell, "GET", &path, None).await;
            assert_eq!(response.status(), 200, "{path}: {}", json_body(&response));
            json_body(&response)
        }
    };
    let names = |page: &Value| -> Vec<String> {
        page["data"]
            .as_array()
            .expect("data")
            .iter()
            .map(|record| record["name"].as_str().expect("name").to_owned())
            .collect()
    };

    // Every object is in the navigation.
    let ui = get(format!("{api}/ui/contributions")).await;
    for object in [
        "custom:account",
        "custom:contact",
        "platform.sales:lead",
        "platform.sales:opportunity",
        "platform.activities:task",
        "platform.activities:event",
    ] {
        assert!(
            ui["navigation"]
                .as_array()
                .expect("navigation")
                .iter()
                .any(|item| item["id"] == format!("objects.{object}")),
            "{object} in {ui}"
        );
    }
    let count = |object: &'static str| {
        let get = &get;
        let api = &api;
        async move {
            get(format!("{api}/objects/{object}/records?limit=100")).await["data"]
                .as_array()
                .map(Vec::len)
        }
    };
    assert_eq!(count("custom:account").await, Some(4));
    assert_eq!(count("custom:contact").await, Some(4));
    assert_eq!(count("platform.sales:lead").await, Some(5));
    assert_eq!(count("platform.sales:opportunity").await, Some(7));
    assert_eq!(count("platform.activities:task").await, Some(5));
    assert_eq!(count("platform.activities:event").await, Some(3));

    // An account: field types, tags and a formula.
    let accounts = get(format!("{api}/objects/custom:account/records?limit=100")).await;
    let analytical = accounts["data"]
        .as_array()
        .expect("accounts")
        .iter()
        .find(|record| record["name"] == "Analytical Engines Ltd")
        .expect("Analytical Engines")
        .clone();
    let id = analytical["id"].as_str().expect("id");
    let record = get(format!("{api}/objects/custom:account/records/{id}")).await;
    assert_eq!(record["custom:segment"], "Enterprise", "{record}");
    assert_eq!(record["custom:tags"], json!(["Key account", "Engines"]));
    assert_eq!(record["custom:industry"], "engineering");

    // Its related lists: contacts and opportunities.
    let related = get(format!("{api}/objects/custom:account/records/{id}/related")).await;
    let text = related.to_string();
    assert!(
        text.contains("Ada Lovelace") && text.contains("Charles Babbage"),
        "{related}"
    );
    assert!(text.contains("Analytical Engine upgrade"), "{related}");

    // Its Activity timeline, and Log a call among its actions.
    let timeline = get(format!(
        "{api}/objects/custom:account/records/{id}/timeline"
    ))
    .await;
    assert!(
        timeline.to_string().contains("Review the service renewal"),
        "{timeline}"
    );
    for item in ["Quarterly business review", "Quarterly check-in call"] {
        assert!(timeline.to_string().contains(item), "{item} in {timeline}");
    }
    let actions = get(format!("{api}/objects/custom:account/quick-actions")).await;
    let labels = actions.to_string();
    for label in ["New contact", "Mark as key account", "Log a call"] {
        assert!(labels.contains(label), "{label} in {actions}");
    }

    // Opportunities: probability follows the stage, and the weighted
    // amount is computed from it.
    let opportunities = get(format!(
        "{api}/objects/platform.sales:opportunity/records?limit=100"
    ))
    .await;
    assert!(names(&opportunities).contains(&"Census tabulators".to_owned()));
    let upgrade = opportunities["data"]
        .as_array()
        .expect("opportunities")
        .iter()
        .find(|record| record["name"] == "Analytical Engine upgrade")
        .expect("upgrade")
        .clone();
    let upgrade_id = upgrade["id"].as_str().expect("id");
    let upgrade = get(format!(
        "{api}/objects/platform.sales:opportunity/records/{upgrade_id}"
    ))
    .await;
    assert_eq!(
        upgrade["platform.sales:probability"].as_f64(),
        Some(80.0),
        "{upgrade}"
    );
    assert_eq!(
        upgrade["custom:weighted_amount"].as_f64(),
        Some(144_000.0),
        "{upgrade}"
    );

    // A contact that cannot be reached is refused by the rule.
    let refused = send(
        &shell,
        "POST",
        &format!("{api}/objects/custom:contact/records"),
        Some(json!({"name": "Nobody"})),
    )
    .await;
    assert_eq!(refused.status(), 400, "{}", json_body(&refused));
    assert!(json_body(&refused)
        .to_string()
        .contains("email address or a phone number"));
}
