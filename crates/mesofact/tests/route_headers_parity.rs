//! R749-F3 / W334 — the **passway** front door's half of the route-header
//! parity fixture (`tests/fixtures/route-headers/parity.json`).
//!
//! The worker door asserts the same fixture through miniflare in
//! `packages/mesofact-edge/tests/route-headers-parity.test.ts`. One fixture,
//! two doors, because the defect this exists to stop is precisely the two doors
//! disagreeing: before this, a domain whose `front_door` flipped `worker` →
//! `passway` kept serving correct bytes with a 200 and silently lost the
//! headers it declared, and cross-origin isolation fails in exactly that
//! shape — `SharedArrayBuffer` becomes undefined and the wasm app throws on
//! load, with nothing server-side to see.

use std::collections::BTreeMap;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use mesofact::{RouteHeaderTable, Server};
use serde::Deserialize;
use tower::ServiceExt;

const PARITY_JSON: &str =
    include_str!("../../../tests/fixtures/route-headers/parity.json");

#[derive(Deserialize)]
struct Parity {
    table: Vec<serde_json::Value>,
    assets: BTreeMap<String, Asset>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Asset {
    body: String,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    path: String,
    status: u16,
    expect: BTreeMap<String, String>,
    #[serde(default)]
    absent: Vec<String>,
}

/// Materialize the fixture's assets as a W272 bundle — the tree the bundle tier
/// actually serves behind passway.
fn bundle_with(assets: &BTreeMap<String, Asset>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("app").join("dist").join("html");
    std::fs::create_dir_all(&html).unwrap();
    for (key, asset) in assets {
        let target = html.join(key);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(target, &asset.body).unwrap();
    }
    std::fs::write(
        dir.path().join("manifest.toml"),
        "schema_version = 1\nname = \"parity-bundle\"\nruntime = \"mesofact/0.8.29\"\n",
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn the_sovereign_door_serves_the_declared_headers_the_worker_serves() {
    let parity: Parity = serde_json::from_str(PARITY_JSON).unwrap();
    assert!(!parity.cases.is_empty(), "fixture has no cases");

    // The table travels as the SAME JSON string the Worker's ROUTE_HEADERS
    // binding carries — that identity is the point, so re-serialize the
    // fixture's `table` rather than hand-building a Rust value.
    let raw = serde_json::to_string(&parity.table).unwrap();
    let table = RouteHeaderTable::parse(&raw).expect("fixture table must parse");

    let bundle = bundle_with(&parity.assets);
    let app = Server::from_bundle(bundle.path())
        .unwrap()
        .with_route_headers(table)
        .router();

    for case in &parity.cases {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&case.path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::from_u16(case.status).unwrap(),
            "{}: {}",
            case.name,
            case.path,
        );
        for (name, value) in &case.expect {
            assert_eq!(
                response
                    .headers()
                    .get(name.to_ascii_lowercase().as_str())
                    .and_then(|v| v.to_str().ok()),
                Some(value.as_str()),
                "{}: {} must carry {name}",
                case.name,
                case.path,
            );
        }
        for name in &case.absent {
            assert!(
                response
                    .headers()
                    .get(name.to_ascii_lowercase().as_str())
                    .is_none(),
                "{}: {} must NOT carry {name} — first match wins, with no merging",
                case.name,
                case.path,
            );
        }

        // The body still arrives: stamping headers must not eat the response.
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!bytes.is_empty(), "{}: empty body", case.name);
    }
}

/// A door with no table declared serves exactly what it served before — the
/// feature is inert unless a domain asks for it.
#[tokio::test]
async fn no_declared_table_leaves_responses_untouched() {
    let parity: Parity = serde_json::from_str(PARITY_JSON).unwrap();
    let bundle = bundle_with(&parity.assets);
    let response = Server::from_bundle(bundle.path())
        .unwrap()
        .router()
        .oneshot(Request::builder().uri("/app").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("cross-origin-opener-policy")
        .is_none());
}
