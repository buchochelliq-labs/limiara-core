//! The route probe on the desktop (ADR 0034 §9): the test plugin declares
//! `device.route_context`, the controller grants exactly that, and each
//! route the web shell reports through the `limiara://` scheme reaches the
//! plugin as ids. The phone builds run the same path in CI, with the
//! plugin's Swift and Kotlin halves on the other side of `deliver`.

use device_route_probe::{Probe, Route};
use device_shell::{probe, Asset, Shell};
use http::Request;
use platform_device::app::{
    AppSettings, Browser, Controller, LoopbackRedirects, MemoryVault, Services,
};
use platform_device::MemoryKeys;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

struct NoBrowser;

impl Browser for NoBrowser {
    fn open(&self, _url: &str) -> Result<(), String> {
        Err("no browser in tests".into())
    }
}

fn shell(directory: &std::path::Path) -> Shell {
    let assets = HashMap::from([(
        "/index.html".to_owned(),
        Asset {
            bytes: b"<div id=\"root\"></div>".to_vec(),
            mime_type: "text/html".into(),
        },
    )]);
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

/// Starts the probe as the app does, with the desktop `Probe` as its
/// native side.
fn start(shell: &Shell) -> Probe {
    let declaration = probe::declaration().expect("declaration");
    let grant = shell.controller().grant(&declaration);
    // It declared the route context and nothing else.
    assert_eq!(
        grant
            .replica_changes()
            .map(|_| ())
            .map_err(|error| error.code()),
        Err("device.capability_not_granted")
    );
    let routes = grant.route_context().expect("granted");
    let plugin = Probe::default();
    let native = plugin.clone();
    tokio::spawn(probe::follow(routes, move |route| {
        native.deliver(route).expect("delivered");
    }));
    plugin
}

async fn received(plugin: &Probe, wanted: &Route) -> Vec<Route> {
    for _ in 0..100 {
        let routes = plugin.received();
        if routes.last() == Some(wanted) {
            return routes;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    plugin.received()
}

#[tokio::test]
async fn the_probe_receives_each_route_the_web_shell_reports() {
    let directory = tempfile::tempdir().expect("dir");
    let shell = shell(directory.path());
    let plugin = start(&shell);

    // Signed out: nothing is shown yet.
    let none = Route::default();
    assert_eq!(received(&plugin, &none).await, vec![none.clone()]);

    // The web shell opens a record page of the demonstration workspace.
    let record = probe::simulate(&shell).await.expect("simulated");
    let page = Route {
        pattern: Some("record".into()),
        object_name: Some("custom:account".into()),
        record_id: Some(record),
    };
    assert_eq!(received(&plugin, &page).await.last(), Some(&page));

    // A message that is not a route is refused, and the probe hears nothing.
    let workspace = device_shell::fixture::WORKSPACE;
    let refused = shell
        .respond(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "limiara://localhost/api/v1/workspaces/{workspace}/ui/route"
                ))
                .header("content-type", "application/json")
                .body(
                    br#"{"route":"record","object":"custom:account","record_id":"<img src=x>"}"#
                        .to_vec(),
                )
                .expect("request"),
        )
        .await;
    assert_eq!(refused.status(), 400);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(plugin.received().last(), Some(&page));

    // Signing out of the demonstration forgets the route.
    let signed_out = shell
        .respond(
            Request::builder()
                .method("DELETE")
                .uri("limiara://localhost/api/device/session")
                .body(Vec::new())
                .expect("request"),
        )
        .await;
    assert_eq!(signed_out.status(), 200);
    assert_eq!(received(&plugin, &none).await.last(), Some(&none));
}

#[tokio::test]
async fn a_simulated_route_runs_when_the_ci_check_asks() {
    // The path a debug phone build takes each second: the check creates the
    // request file in the probe's directory, the shell consumes it, drives
    // a record page through the scheme, and hands the outcome line to the
    // probe's native half for its trail.
    let directory = tempfile::tempdir().expect("dir");
    let shell = shell(directory.path());
    let plugin = start(&shell);
    let probe_directory = directory.path().join("probe");
    std::fs::create_dir_all(&probe_directory).expect("probe dir");

    // Nothing asked: nothing runs.
    assert_eq!(
        probe::simulate_if_asked(&shell, &probe_directory).await,
        None
    );
    assert!(shell.controller().bridge().is_none());

    let request = probe_directory.join(probe::SIMULATE_FILE);
    std::fs::write(&request, b"").expect("asked");
    let line = probe::simulate_if_asked(&shell, &probe_directory)
        .await
        .expect("asked once");
    let record = line
        .strip_prefix("simulated a route to ")
        .unwrap_or_else(|| panic!("the simulated route failed: {line}"))
        .to_owned();
    assert!(!request.exists(), "the request is taken once");
    let page = Route {
        pattern: Some("record".into()),
        object_name: Some("custom:account".into()),
        record_id: Some(record),
    };
    assert_eq!(received(&plugin, &page).await.last(), Some(&page));
    // The line the phone scripts look for, and the route line's ids.
    assert!(line.starts_with("simulated a route to "));
    assert_eq!(page.record_id.as_deref().map(str::len), Some(18));

    // Asked again with the demonstration already open: it reuses it.
    std::fs::write(&request, b"").expect("asked again");
    let again = probe::simulate_if_asked(&shell, &probe_directory)
        .await
        .expect("asked twice");
    assert!(again.starts_with("simulated a route to "), "{again}");
    assert_eq!(
        probe::simulate_if_asked(&shell, &probe_directory).await,
        None
    );
}
