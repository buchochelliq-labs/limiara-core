//! The `limiara://` scheme handler, as the webview sees it: the embedded web
//! build, the shell's routes, and the API answered by the bridge over the
//! demonstration workspace. The requests are those the web shell makes on
//! start (`apps/web/src/shell/data.ts`) and on an object's pages
//! (`apps/web/src/objects/api.ts`).

use device_shell::{fixture, Asset, Shell, CONTENT_SECURITY_POLICY};
use http::{Request, Response};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

fn asset(body: &str, mime_type: &str) -> Asset {
    Asset {
        bytes: body.as_bytes().to_vec(),
        mime_type: mime_type.into(),
    }
}

async fn open_shell(directory: &std::path::Path) -> Shell {
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
    let bridge = fixture::open(directory).await.expect("demo workspace");
    Shell::new(bridge, Arc::new(assets))
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
    assert_eq!(page["data"].as_array().map(Vec::len), Some(3), "{page}");

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
    assert_eq!(page["data"].as_array().map(Vec::len), Some(3), "{page}");
}
