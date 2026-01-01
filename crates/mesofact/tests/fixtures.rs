//! Runs every `*.json` in `tests/fixtures/manifests/` (workspace-shared with
//! the TS validator) and asserts the Rust validator's verdict matches the
//! fixture's `expect` field.

use mesofact::{validate, Manifest, SourceCatalog, SourceScope};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum FixtureScope {
    Global,
    Project,
    User,
}

impl From<FixtureScope> for SourceScope {
    fn from(s: FixtureScope) -> Self {
        match s {
            FixtureScope::Global => SourceScope::Global,
            FixtureScope::Project => SourceScope::Project,
            FixtureScope::User => SourceScope::User,
        }
    }
}

#[derive(Deserialize)]
struct SourceEntry {
    scope: FixtureScope,
}

#[derive(Deserialize)]
struct ExpectedError {
    kind: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Expect {
    Ok(String), // literal "ok"
    Errors { errors: Vec<ExpectedError> },
}

#[derive(Deserialize)]
struct Fixture {
    sources: std::collections::BTreeMap<String, SourceEntry>,
    manifest: Manifest,
    expect: Expect,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("manifests")
}

fn load(path: &Path) -> Fixture {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn run(path: &Path) {
    let fx = load(path);
    let catalog: SourceCatalog = fx
        .sources
        .into_iter()
        .map(|(k, v)| (k, SourceScope::from(v.scope)))
        .collect();

    let result = validate(&fx.manifest, &catalog);

    match fx.expect {
        Expect::Ok(s) if s == "ok" => assert!(
            result.is_ok(),
            "{}: expected ok, got {:?}",
            path.display(),
            result
        ),
        Expect::Ok(other) => panic!("{}: unknown expect literal '{other}'", path.display()),
        Expect::Errors { errors: expected } => {
            let errs = result.expect_err(&format!(
                "{}: expected errors, got Ok",
                path.display()
            ));
            let got: BTreeSet<&str> = errs.iter().map(|e| e.kind.label()).collect();
            let want: BTreeSet<&str> = expected.iter().map(|e| e.kind.as_str()).collect();
            assert_eq!(
                got, want,
                "{}: error kinds mismatch (got {got:?}, want {want:?})",
                path.display()
            );
        }
    }
}

#[test]
fn shared_fixtures() {
    let dir = fixtures_dir();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fixtures found in {}", dir.display());
    for path in paths {
        run(&path);
    }
}
