//! The shell's side of the route probe (`route-probe/`, ADR 0034 §9): the
//! test plugin that receives the current-route context on desktop, iOS and
//! Android.
//!
//! - [`declaration`] is what the plugin declares, `device.route_context`
//!   only; the shell grants it through the controller, which checks the
//!   declaration before the plugin can subscribe.
//! - [`follow`] hands each route change to the plugin, as ids.
//! - [`simulate`] drives a route through the shell exactly as the web view
//!   would, through the `limiara://` scheme: it opens the demonstration
//!   workspace if nothing is open, then reports one of its records.
//!   [`simulate_if_asked`] runs it when the CI check asks, since a CI runner
//!   cannot tap through the web shell: debug phone builds call it every
//!   second (`app.rs`) and hand the line it returns to the probe's native
//!   half, the trail's only writer. The desktop tests call both directly.

use crate::Shell;
use device_route_probe::Route;
use http::Request;
use platform_device::capabilities::{CapabilityError, PluginDeclaration};
use platform_device::route::{CurrentRoute, RouteSubscription};
use serde_json::{json, Value};
use std::path::Path;

/// The file that asks a debug phone build for a simulated route, in the
/// probe's directory.
pub const SIMULATE_FILE: &str = "simulate-route";

/// What the route probe declares.
pub fn declaration() -> Result<PluginDeclaration, CapabilityError> {
    PluginDeclaration::new(device_route_probe::NAME, device_route_probe::CAPABILITIES)
}

/// A route as the probe's native half takes it.
pub fn probe_route(route: Option<&CurrentRoute>) -> Route {
    match route {
        Some(route) => Route {
            pattern: serde_json::to_value(route.route)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned)),
            object_name: route.object.clone(),
            record_id: route.record_id.clone(),
        },
        None => Route::default(),
    }
}

/// Hands every route change to `deliver` until the route context ends with
/// the app, starting with the route as it is now. A delivery that fails is
/// passed over: the next change tries again.
pub async fn follow(mut routes: RouteSubscription, deliver: impl Fn(&Route)) {
    while let Ok(route) = routes.changed().await {
        deliver(&probe_route(route.as_ref()));
    }
}

async fn call(
    shell: &Shell,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("limiara://localhost{path}"));
    let bytes = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            body.to_string().into_bytes()
        }
        None => Vec::new(),
    };
    let request = request
        .body(bytes)
        .map_err(|error| format!("the request could not be built: {error}"))?;
    let response = shell.respond(request).await;
    if !response.status().is_success() {
        return Err(format!(
            "{method} {path} answered {}",
            response.status().as_u16()
        ));
    }
    if response.body().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(response.body()).map_err(|error| format!("{path}: {error}"))
}

/// Reports a record page of the open workspace's first object through the
/// shell, as the web shell does when it navigates; with no workspace open,
/// opens the demonstration workspace first. Returns the record id.
pub async fn simulate(shell: &Shell) -> Result<String, String> {
    if shell.controller().bridge().is_none() {
        call(
            shell,
            "POST",
            crate::SESSION_PATH,
            Some(json!({"demo": true})),
        )
        .await?;
    }
    let workspaces = call(shell, "GET", "/api/v1/workspaces", None).await?;
    let workspace = workspaces["data"][0]["id"]
        .as_str()
        .ok_or("no workspace is open")?
        .to_owned();
    let base = format!("/api/v1/workspaces/{workspace}");
    let ui = call(shell, "GET", &format!("{base}/ui/contributions"), None).await?;
    if ui["route_context"] != Value::Bool(true) {
        return Err("the host does not take route messages".into());
    }
    let objects = call(shell, "GET", &format!("{base}/objects"), None).await?;
    let object = objects["data"][0]["api_name"]
        .as_str()
        .ok_or("the workspace has no object")?
        .to_owned();
    let records = call(
        shell,
        "GET",
        &format!("{base}/objects/{object}/records"),
        None,
    )
    .await?;
    let record = records["data"][0]["id"]
        .as_str()
        .ok_or("the object has no record")?
        .to_owned();
    call(
        shell,
        "POST",
        &format!("{base}/ui/route"),
        Some(json!({"route": "record", "object": object, "record_id": record})),
    )
    .await?;
    Ok(record)
}

/// Runs [`simulate`] once if `directory` holds [`SIMULATE_FILE`], removing
/// it, and returns the line for the probe's trail: `simulated a route to …`
/// or `the simulated route failed: …`. `None` when nothing was asked.
pub async fn simulate_if_asked(shell: &Shell, directory: &Path) -> Option<String> {
    std::fs::remove_file(directory.join(SIMULATE_FILE)).ok()?;
    Some(match simulate(shell).await {
        Ok(record) => format!("simulated a route to {record}"),
        Err(error) => format!("the simulated route failed: {error}"),
    })
}
