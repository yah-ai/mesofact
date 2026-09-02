//! R749-B4 — the Rust half of the `cache_policy` parity fixture
//! (`tests/fixtures/cache-policy/parity.json`).
//!
//! The edge half asserts the same fixture in
//! `packages/mesofact-edge/tests/cache-policy-parity.test.ts`. Three tiers turn
//! a declared `cache_policy` into a header — the serve tier's middleware, the
//! publisher's PUT, and the Worker — and the defect this exists to stop is any
//! two of them disagreeing. That is not hypothetical: before R749-B4 the
//! publisher chose by path prefix and the Worker overwrote every page with a
//! flat `no-cache`, so `{ ttl: 3600 }` was honoured by both servers and by
//! nothing in front of them.
//!
//! Both Rust entry points are asserted here against one fixture row, so the
//! object's header and the origin's header cannot come apart either.

use std::collections::BTreeMap;

use axum::http::{header, HeaderMap, StatusCode};
use mesofact_core::CachePolicyTable;
use serde::Deserialize;

const PARITY_JSON: &str = include_str!("../../../tests/fixtures/cache-policy/parity.json");

#[derive(Deserialize)]
struct Parity {
    routes: Vec<serde_json::Value>,
    cases: Vec<Case>,
    negative_cases: Vec<NegativeCase>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    path: String,
    cache_control: Option<String>,
    #[serde(default)]
    vary: Option<String>,
}

#[derive(Deserialize)]
struct NegativeCase {
    name: String,
    path: String,
    status: u16,
    cache_control: Option<String>,
}

fn fixture() -> (Parity, CachePolicyTable) {
    let parity: Parity = serde_json::from_str(PARITY_JSON).expect("fixture parses");
    // Build the table from the SAME manifest JSON the edge reads, rather than
    // hand-constructing Rust values: the identity of the input is the point.
    let manifest = serde_json::json!({ "routes": parity.routes });
    let table = CachePolicyTable::from_manifest_json(manifest.to_string().as_bytes())
        .expect("fixture routes build a table");
    (parity, table)
}

fn applied(table: &CachePolicyTable, path: &str, status: StatusCode) -> BTreeMap<String, String> {
    let mut headers = HeaderMap::new();
    table.apply(path, status, &mut headers);
    headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap().to_string()))
        .collect()
}

#[test]
fn the_serve_tier_emits_what_the_fixture_declares() {
    let (parity, table) = fixture();
    assert!(!parity.cases.is_empty(), "fixture has no cases");

    for case in &parity.cases {
        let headers = applied(&table, &case.path, StatusCode::OK);
        assert_eq!(
            headers.get(header::CACHE_CONTROL.as_str()).map(String::as_str),
            case.cache_control.as_deref(),
            "{}: {}",
            case.name,
            case.path,
        );
        assert_eq!(
            headers.get(header::VARY.as_str()).map(String::as_str),
            case.vary.as_deref(),
            "{}: {} Vary",
            case.name,
            case.path,
        );
    }
}

/// The publish path reads the same table through a header-free accessor, and
/// must answer the same string. A page published with one TTL and served with
/// another is the R749-B4 defect wearing a different hat.
#[test]
fn the_publish_tier_agrees_with_the_serve_tier() {
    let (parity, table) = fixture();
    for case in &parity.cases {
        assert_eq!(
            table.cache_control_for(&case.path),
            case.cache_control.as_deref(),
            "{}: {}",
            case.name,
            case.path,
        );
    }
}

/// `negative_ttl` is a Rust-tier row — see the fixture README for why the edge
/// does not carry it.
#[test]
fn a_miss_is_cached_only_when_the_route_said_so() {
    let (parity, table) = fixture();
    assert!(!parity.negative_cases.is_empty(), "fixture has no negative cases");

    for case in &parity.negative_cases {
        let status = StatusCode::from_u16(case.status).unwrap();
        let headers = applied(&table, &case.path, status);
        assert_eq!(
            headers.get(header::CACHE_CONTROL.as_str()).map(String::as_str),
            case.cache_control.as_deref(),
            "{}: {} @ {}",
            case.name,
            case.path,
            case.status,
        );
    }
}
