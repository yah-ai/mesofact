// CLI smoke for `mesofact-publish`. Asserts the binary parses args, drives
// publish_dist against an in-memory backend, and exits 0 with `--in-memory`.
//
// T7 swapped in real S3 + Cloudflare adapters: without `--in-memory` the
// binary now loads `mesofact.config.toml`. Missing config or missing
// credentials exits 2 with a precise hint (vs. silently trying to publish).

use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

const BIN: &str = env!("CARGO_BIN_EXE_mesofact-publish");
const BUILD_ID: &str = "2026-05-15T17-00-00Z";

fn write_minimal_dist(dir: &Path) {
    let manifest = serde_json::json!({
        "version": "1",
        "build_id": BUILD_ID,
        "routes": [{
            "route": "/",
            "mode": "static",
            "render_entrypoint": "dist/server/home.js",
            "cache_policy": { "ttl": 86400 }
        }],
        "static_assets": []
    });
    let tag_index = serde_json::json!({
        "build_id": BUILD_ID,
        "tags": {}
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(dir.join("tag-index.json"), tag_index.to_string()).unwrap();
    std::fs::create_dir_all(dir.join("server")).unwrap();
    std::fs::write(dir.join("server/home.js"), b"export const render = () => null;\n").unwrap();
}

#[test]
fn in_memory_flag_publishes_ok() {
    let dir = tempdir().unwrap();
    write_minimal_dist(dir.path());

    let out = Command::new(BIN)
        .arg(dir.path())
        .arg("--in-memory")
        .output()
        .expect("spawn mesofact-publish");
    assert!(
        out.status.success(),
        "mesofact-publish failed: status={:?}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("publish ok"), "stdout: {stdout}");
    assert!(stdout.contains(BUILD_ID), "stdout: {stdout}");
}

#[test]
fn missing_config_without_in_memory_exits_with_hint() {
    let dir = tempdir().unwrap();
    write_minimal_dist(dir.path());

    let out = Command::new(BIN)
        .arg(dir.path())
        .arg("--config")
        .arg(dir.path().join("does-not-exist.toml"))
        .output()
        .expect("spawn mesofact-publish");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--in-memory") || stderr.contains("mesofact.config.toml"),
        "stderr: {stderr}"
    );
}

#[test]
fn missing_publish_block_exits_with_hint() {
    let dir = tempdir().unwrap();
    write_minimal_dist(dir.path());
    let cfg = dir.path().join("mesofact.config.toml");
    std::fs::write(
        &cfg,
        "[sources.foo]\nkind = \"r2\"\nbucket = \"x\"\nendpoint = \"y\"\n",
    )
    .unwrap();
    let out = Command::new(BIN)
        .arg(dir.path())
        .arg("--config")
        .arg(&cfg)
        .output()
        .expect("spawn mesofact-publish");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[publish]"), "stderr: {stderr}");
}

#[test]
fn missing_credentials_env_exits_with_hint() {
    let dir = tempdir().unwrap();
    write_minimal_dist(dir.path());
    let cfg = dir.path().join("mesofact.config.toml");
    std::fs::write(
        &cfg,
        r#"
[publish]
bucket = "smoke"
endpoint = "https://example.invalid"
zone_id = "zone"
access_key_id_env = "MESOFACT_TEST_NEVER_SET_AKID"
secret_access_key_env = "MESOFACT_TEST_NEVER_SET_SECRET"
api_token_env = "MESOFACT_TEST_NEVER_SET_TOKEN"
"#,
    )
    .unwrap();
    let out = Command::new(BIN)
        .arg(dir.path())
        .arg("--config")
        .arg(&cfg)
        .env_remove("MESOFACT_TEST_NEVER_SET_AKID")
        .env_remove("MESOFACT_TEST_NEVER_SET_SECRET")
        .env_remove("MESOFACT_TEST_NEVER_SET_TOKEN")
        .output()
        .expect("spawn mesofact-publish");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("MESOFACT_TEST_NEVER_SET_AKID"),
        "stderr: {stderr}"
    );
}

#[test]
fn missing_manifest_exits_nonzero() {
    let dir = tempdir().unwrap();
    let out = Command::new(BIN)
        .arg(dir.path())
        .arg("--in-memory")
        .output()
        .expect("spawn mesofact-publish");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("manifest.json"), "stderr: {stderr}");
}
