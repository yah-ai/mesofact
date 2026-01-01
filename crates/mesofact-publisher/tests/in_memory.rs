// In-memory smoke tests for the publisher orchestrator. Covers trait contracts
// + the T1 happy path; T2/T3/T4 will add idempotency, tag-diff purge, and pin
// rollback behavior on top.

use bytes::Bytes;
use mesofact_publisher::{
    publish_dist, publish_pin, CdnPurger, InMemoryPurger, InMemoryStore, ObjectStore, PublishError,
    PutOpts,
};
use std::path::Path;
use tempfile::tempdir;
use tokio::fs;

const BUILD_ID: &str = "2026-05-15T17-00-00Z";

fn manifest_json(build_id: &str) -> String {
    serde_json::json!({
        "version": "1",
        "build_id": build_id,
        "routes": [{
            "route": "/",
            "mode": "static",
            "render_entrypoint": "dist/server/home.js",
            "cache_policy": { "ttl": 86400 }
        }],
        "static_assets": []
    })
    .to_string()
}

fn tag_index_json(build_id: &str) -> String {
    serde_json::json!({
        "build_id": build_id,
        "tags": {
            "r2:assets:logo.svg": ["/"]
        }
    })
    .to_string()
}

async fn write_dist(dir: &Path, build_id: &str) {
    fs::write(dir.join("manifest.json"), manifest_json(build_id))
        .await
        .unwrap();
    fs::write(dir.join("tag-index.json"), tag_index_json(build_id))
        .await
        .unwrap();
    fs::create_dir_all(dir.join("server")).await.unwrap();
    fs::write(
        dir.join("server/home.js"),
        b"export const render = () => ({ html: '<h1>hi</h1>', cache: { ttl: 86400 } });\n",
    )
    .await
    .unwrap();
    fs::create_dir_all(dir.join("html")).await.unwrap();
    fs::write(dir.join("html/home.html"), b"<!doctype html><h1>hi</h1>\n")
        .await
        .unwrap();
}

#[tokio::test]
async fn in_memory_store_round_trip() {
    let store = InMemoryStore::new();
    assert!(store.is_empty());

    let body = Bytes::from_static(b"hello");
    store
        .put(
            "a/b.txt",
            body.clone(),
            PutOpts {
                content_type: "text/plain".into(),
                content_hash: "deadbeef".into(),
                cache_control: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(store.get("a/b.txt").await.unwrap().as_deref(), Some(&body[..]));
    let meta = store.head("a/b.txt").await.unwrap().unwrap();
    assert_eq!(meta.content_hash, "deadbeef");
    assert_eq!(meta.content_type, "text/plain");
    assert_eq!(meta.size, 5);

    assert_eq!(store.list("a/").await.unwrap(), vec!["a/b.txt".to_string()]);
    assert_eq!(store.list("z/").await.unwrap(), Vec::<String>::new());

    store.delete("a/b.txt").await.unwrap();
    assert!(store.get("a/b.txt").await.unwrap().is_none());
}

#[tokio::test]
async fn in_memory_purger_records_calls() {
    let purger = InMemoryPurger::new();
    purger
        .purge_tags(&["r2:assets:logo.svg".into(), "sqlite:db:p:1".into()])
        .await
        .unwrap();
    purger.purge_tags(&[]).await.unwrap(); // empty calls are dropped
    purger.purge_tags(&["sqlite:db:p:1".into()]).await.unwrap();

    assert_eq!(purger.calls().len(), 2);
    assert_eq!(
        purger.flat_tags(),
        vec!["r2:assets:logo.svg".to_string(), "sqlite:db:p:1".into()]
    );
}

#[tokio::test]
async fn publish_dist_uploads_artifacts_and_pointers() {
    let dir = tempdir().unwrap();
    write_dist(dir.path(), BUILD_ID).await;

    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();
    let report = publish_dist(dir.path(), &store, &purger).await.unwrap();

    assert_eq!(report.build_id, BUILD_ID);
    assert!(report.uploaded_keys.contains(&"manifest.json".to_string()));
    assert!(report.uploaded_keys.contains(&"tag-index.json".to_string()));
    assert!(report.uploaded_keys.contains(&format!("{BUILD_ID}/manifest.json")));
    assert!(report.uploaded_keys.contains(&format!("{BUILD_ID}/server/home.js")));
    assert!(report.uploaded_keys.contains(&format!("{BUILD_ID}/html/home.html")));

    // Pointer at root matches the build's manifest.
    let live_manifest = store.get("manifest.json").await.unwrap().unwrap();
    assert_eq!(&live_manifest[..], manifest_json(BUILD_ID).as_bytes());

    // Per-build snapshot is also present.
    let snapshot = store
        .get(&format!("{BUILD_ID}/manifest.json"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&snapshot[..], manifest_json(BUILD_ID).as_bytes());

    // Cache-Control: assets/* and hydrate/* immutable, html/* long, server/* short,
    // pointers no-cache.
    let pointer_meta = store.head("manifest.json").await.unwrap().unwrap();
    // We don't expose cache_control on ObjectMeta — content-type is enough to
    // smoke that the right PutOpts flowed through.
    assert_eq!(pointer_meta.content_type, "application/json");

    // T1 has no CDN purge; T3 will populate this.
    assert!(report.purged_tags.is_empty());
    assert!(purger.flat_tags().is_empty());
}

#[tokio::test]
async fn publish_dist_is_idempotent_on_unchanged_dist() {
    let dir = tempdir().unwrap();
    write_dist(dir.path(), BUILD_ID).await;

    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();

    let first = publish_dist(dir.path(), &store, &purger).await.unwrap();
    assert!(!first.uploaded_keys.is_empty(), "first run should upload");
    assert!(
        first.skipped_keys.is_empty(),
        "fresh store has nothing to skip"
    );
    let store_size_after_first = store.len();

    let second = publish_dist(dir.path(), &store, &purger).await.unwrap();
    assert!(
        second.uploaded_keys.is_empty(),
        "republish of unchanged dist should upload nothing, got {:?}",
        second.uploaded_keys
    );
    // Every key the first run touched should be reported skipped on the
    // second — same artifacts, same per-build snapshots, same root pointers.
    let mut want_skipped = first.uploaded_keys.clone();
    want_skipped.sort();
    let mut got_skipped = second.skipped_keys.clone();
    got_skipped.sort();
    assert_eq!(got_skipped, want_skipped);
    // Store contents unchanged across the no-op republish.
    assert_eq!(store.len(), store_size_after_first);
}

#[tokio::test]
async fn publish_dist_purges_only_changed_tags_on_rerun() {
    let dir = tempdir().unwrap();

    // Two-route build: "/" tagged r2:assets:logo.svg, "/about" tagged
    // r2:assets:about.md. The "/about" route is the one we'll change.
    let two_route_manifest = serde_json::json!({
        "version": "1",
        "build_id": BUILD_ID,
        "routes": [
            { "route": "/", "mode": "static", "render_entrypoint": "dist/server/home.js",
              "cache_policy": { "ttl": 86400 } },
            { "route": "/about", "mode": "static", "render_entrypoint": "dist/server/about.js",
              "cache_policy": { "ttl": 86400 } }
        ],
        "static_assets": []
    })
    .to_string();
    let initial_tag_index = serde_json::json!({
        "build_id": BUILD_ID,
        "tags": {
            "r2:assets:logo.svg": ["/"],
            "r2:assets:about.md": ["/about"]
        }
    })
    .to_string();

    fs::write(dir.path().join("manifest.json"), &two_route_manifest)
        .await
        .unwrap();
    fs::write(dir.path().join("tag-index.json"), &initial_tag_index)
        .await
        .unwrap();
    fs::create_dir_all(dir.path().join("server")).await.unwrap();
    fs::write(dir.path().join("server/home.js"), b"home v1\n")
        .await
        .unwrap();
    fs::write(dir.path().join("server/about.js"), b"about v1\n")
        .await
        .unwrap();
    fs::create_dir_all(dir.path().join("html")).await.unwrap();
    fs::write(dir.path().join("html/home.html"), b"<h1>home</h1>\n")
        .await
        .unwrap();
    fs::write(dir.path().join("html/about.html"), b"<h1>about v1</h1>\n")
        .await
        .unwrap();

    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();

    // First publish: no prior tag-index in the store → empty purge.
    let first = publish_dist(dir.path(), &store, &purger).await.unwrap();
    assert!(first.purged_tags.is_empty(), "first publish should not purge");
    assert!(purger.flat_tags().is_empty());

    // Change /about's HTML; bump its tag's URL set (e.g. add a fragment) and
    // leave logo's mapping untouched. r2:assets:about.md is the only tag
    // whose URL list materially changed.
    let new_tag_index = serde_json::json!({
        "build_id": BUILD_ID,
        "tags": {
            "r2:assets:logo.svg": ["/"],
            "r2:assets:about.md": ["/about", "/about#section"]
        }
    })
    .to_string();
    fs::write(dir.path().join("tag-index.json"), &new_tag_index)
        .await
        .unwrap();
    fs::write(dir.path().join("html/about.html"), b"<h1>about v2</h1>\n")
        .await
        .unwrap();

    let second = publish_dist(dir.path(), &store, &purger).await.unwrap();
    assert_eq!(
        second.purged_tags,
        vec!["r2:assets:about.md".to_string()],
        "only the route whose tag URL set changed should be purged"
    );
    assert_eq!(
        purger.flat_tags(),
        vec!["r2:assets:about.md".to_string()],
        "purger should have seen exactly the one diffed tag"
    );
}

#[tokio::test]
async fn publish_dist_purges_added_and_removed_tags() {
    let dir = tempdir().unwrap();
    write_dist(dir.path(), BUILD_ID).await;

    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();
    publish_dist(dir.path(), &store, &purger).await.unwrap();
    // After first publish, prior tag-index in store is {logo.svg: ["/"]}.

    // Swap the single tag for a new one — old must be evicted (prior cached
    // HTML is still tagged with it), new must be flagged (newly tracked).
    let swapped = serde_json::json!({
        "build_id": BUILD_ID,
        "tags": { "r2:assets:hero.png": ["/"] }
    })
    .to_string();
    fs::write(dir.path().join("tag-index.json"), &swapped)
        .await
        .unwrap();

    let report = publish_dist(dir.path(), &store, &purger).await.unwrap();
    assert_eq!(
        report.purged_tags,
        vec![
            "r2:assets:hero.png".to_string(),
            "r2:assets:logo.svg".to_string(),
        ],
        "both added and removed tags should land in the purge set (sorted)"
    );
}

#[tokio::test]
async fn publish_dist_uploads_only_changed_artifacts_on_rerun() {
    let dir = tempdir().unwrap();
    write_dist(dir.path(), BUILD_ID).await;

    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();
    publish_dist(dir.path(), &store, &purger).await.unwrap();

    // Mutate one artifact, keep the same build_id (republish-in-place case).
    fs::write(
        dir.path().join("html/home.html"),
        b"<!doctype html><h1>hi v2</h1>\n",
    )
    .await
    .unwrap();

    let report = publish_dist(dir.path(), &store, &purger).await.unwrap();
    let changed_key = format!("{BUILD_ID}/html/home.html");
    assert!(
        report.uploaded_keys.contains(&changed_key),
        "changed artifact must be re-uploaded; got {:?}",
        report.uploaded_keys
    );
    // The unchanged server bundle should be skipped.
    let unchanged_key = format!("{BUILD_ID}/server/home.js");
    assert!(
        report.skipped_keys.contains(&unchanged_key),
        "unchanged artifact must be skipped; got {:?}",
        report.skipped_keys
    );
}

#[tokio::test]
async fn publish_dist_rejects_build_id_mismatch() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("manifest.json"), manifest_json("aaa"))
        .await
        .unwrap();
    fs::write(dir.path().join("tag-index.json"), tag_index_json("bbb"))
        .await
        .unwrap();

    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();
    let err = publish_dist(dir.path(), &store, &purger).await.unwrap_err();
    assert!(matches!(err, PublishError::Parse(_)), "got {err:?}");
    // Nothing should land in the store on a validation failure.
    assert!(store.is_empty());
}

#[tokio::test]
async fn publish_dist_missing_manifest_is_typed_error() {
    let dir = tempdir().unwrap();
    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();
    let err = publish_dist(dir.path(), &store, &purger).await.unwrap_err();
    assert!(matches!(err, PublishError::ManifestMissing(_)), "got {err:?}");
}

#[tokio::test]
async fn publish_pin_restores_prior_build() {
    let dir = tempdir().unwrap();
    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();

    // Build A is the current pointer.
    write_dist(dir.path(), "build-a").await;
    publish_dist(dir.path(), &store, &purger).await.unwrap();

    // Build B overwrites the pointer.
    fs::write(dir.path().join("manifest.json"), manifest_json("build-b"))
        .await
        .unwrap();
    fs::write(dir.path().join("tag-index.json"), tag_index_json("build-b"))
        .await
        .unwrap();
    publish_dist(dir.path(), &store, &purger).await.unwrap();

    assert_eq!(
        store.get("manifest.json").await.unwrap().unwrap(),
        Bytes::from(manifest_json("build-b")),
    );

    // Pin back to build A — pointer flips, both per-build snapshots remain,
    // and the about-to-be-stale-build-B HTML is evicted from the CDN by
    // purging every tag the now-live (pre-pin) tag-index carried.
    let purger_calls_before = purger.calls().len();
    let report = publish_pin("build-a", &store, &purger).await.unwrap();
    assert_eq!(report.build_id, "build-a");
    assert_eq!(
        store.get("manifest.json").await.unwrap().unwrap(),
        Bytes::from(manifest_json("build-a")),
    );
    assert!(store.get("build-a/manifest.json").await.unwrap().is_some());
    assert!(store.get("build-b/manifest.json").await.unwrap().is_some());
    assert_eq!(
        report.purged_tags,
        vec!["r2:assets:logo.svg".to_string()],
        "pin should purge the currently-live tag-index's tag set"
    );
    let new_call = &purger.calls()[purger_calls_before];
    assert_eq!(new_call, &vec!["r2:assets:logo.svg".to_string()]);
}

#[tokio::test]
async fn publish_pin_purges_full_live_tag_set() {
    let dir = tempdir().unwrap();
    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();

    // Build A — single-tag index.
    write_dist(dir.path(), "build-a").await;
    publish_dist(dir.path(), &store, &purger).await.unwrap();

    // Build B — two distinct tags. After this publish, the live tag-index in
    // the store carries both. Pinning back to A must purge both, regardless
    // of whether they appear in A's index.
    fs::write(dir.path().join("manifest.json"), manifest_json("build-b"))
        .await
        .unwrap();
    let two_tag_index = serde_json::json!({
        "build_id": "build-b",
        "tags": {
            "r2:assets:hero.png": ["/"],
            "sqlite:db:page:1": ["/p/1"]
        }
    })
    .to_string();
    fs::write(dir.path().join("tag-index.json"), two_tag_index)
        .await
        .unwrap();
    publish_dist(dir.path(), &store, &purger).await.unwrap();

    let report = publish_pin("build-a", &store, &purger).await.unwrap();
    assert_eq!(
        report.purged_tags,
        vec![
            "r2:assets:hero.png".to_string(),
            "sqlite:db:page:1".to_string(),
        ],
        "pin must purge every tag the rolled-away-from build carried (sorted)"
    );
}

#[tokio::test]
async fn publish_pin_with_no_live_index_skips_purge() {
    // Edge case: the per-build snapshots are present but no root pointer was
    // ever written (e.g. publish crashed before commit). Pin should still
    // recover the pointer; there's no live cached HTML to evict.
    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();

    // Hand-seed only the per-build snapshot.
    store
        .put(
            "build-a/manifest.json",
            Bytes::from(manifest_json("build-a")),
            mesofact_publisher::PutOpts {
                content_type: "application/json".into(),
                content_hash: "x".into(),
                cache_control: None,
            },
        )
        .await
        .unwrap();
    store
        .put(
            "build-a/tag-index.json",
            Bytes::from(tag_index_json("build-a")),
            mesofact_publisher::PutOpts {
                content_type: "application/json".into(),
                content_hash: "y".into(),
                cache_control: None,
            },
        )
        .await
        .unwrap();

    let report = publish_pin("build-a", &store, &purger).await.unwrap();
    assert!(report.purged_tags.is_empty());
    assert!(purger.calls().is_empty());
    // Root pointer landed.
    assert!(store.get("manifest.json").await.unwrap().is_some());
}

#[tokio::test]
async fn publish_pin_unknown_build_id_is_typed_error() {
    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();
    let err = publish_pin("never-existed", &store, &purger)
        .await
        .unwrap_err();
    assert!(matches!(err, PublishError::PinNotFound(_)), "got {err:?}");
}
