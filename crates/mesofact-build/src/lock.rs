//! The seam a resolver writes a lockfile through (R771-T5, W319 §6 P4).
//!
//! Stage 2 (W318) will resolve semver ranges and then have to *record* what it
//! resolved. This module defines how, while the materializer in
//! [`crate::install`] is still the only consumer — which is the cheapest
//! moment to get the shape right and the last moment before a resolver starts
//! guessing at it.
//!
//! # The one rule
//!
//! **Nothing here names a concept the materializer cannot observe.** The seam
//! carries an install path and a [`PackageSource`] — the same
//! `Registry{name, version, integrity} | Link{target}` vocabulary the three
//! parsers already reduce their formats to. No ranges, no peer relationships,
//! no resolution provenance. Those are real things a resolver knows, and a
//! lock that carried them would be a lock only that resolver could write; a
//! lock that carries only what a materializer reads can be written by hand, by
//! a competitor, or by `bun install`.
//!
//! # Why bun.lock is the format
//!
//! W318 §6 phase 3: bun.lock's `packages` keys are install *paths*, so a peer
//! instance nested under its requester (`@dnd-kit/core/react`) is expressible
//! with no format change — which is exactly the case pnpm's format needs a
//! whole derivation for (see [`crate::pnpm`]). The reader already exists
//! (`parse_bun_lock`), so the seam is proved by round-tripping through it
//! rather than by a spec.
//!
//! # Deliberately not written
//!
//! bun's own writer puts a package's declared `dependencies`,
//! `peerDependencies` and `bin` in the third slot of each entry. This writes
//! `{}` there, because a materializer never reads it and the rule above says
//! the seam does not carry what the materializer cannot observe. The
//! consequence, stated so nobody discovers it: the `bun` CLI reading one of
//! our lockfiles would re-resolve the graph rather than trust it. Our reader
//! ignores the slot entirely.

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::install::PackageSource;

/// One decided placement: where it goes, and what goes there.
///
/// `path` is relative to the project's **root `node_modules`**, which is
/// bun.lock's key form — `react`, `@scope/pkg`, and
/// `@scope/pkg/react` for a copy nested under `@scope/pkg`. That last shape
/// is what makes a peer instance expressible without a format change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    pub path: String,
    pub source: PackageSource,
}

/// How a resolver hands a decided tree to a lockfile format.
///
/// Rendering is separate from writing so a caller can diff, print or test a
/// lock without touching a filesystem — and so this trait has no IO in it to
/// mock.
pub trait LockWriter {
    /// What the rendered lock must be saved as for the installer to find it.
    fn file_name(&self) -> &'static str;
    /// Serialize `entries`. Deterministic: the same tree renders byte-identical.
    fn render(&self, entries: &[LockEntry]) -> Result<String>;
}

/// Render and save into `project_root`, returning the path written.
pub fn write_lock(
    writer: &dyn LockWriter,
    project_root: &Path,
    entries: &[LockEntry],
) -> Result<PathBuf> {
    let path = project_root.join(writer.file_name());
    std::fs::write(&path, writer.render(entries)?)?;
    Ok(path)
}

/// Writes `bun.lock` v1.
///
/// Strict JSON, not the JSONC bun itself emits (trailing commas): a strict
/// document is a valid input to every reader that accepts the loose one,
/// including ours.
pub struct BunLockWriter {
    /// The root project's `name`, which bun.lock records under `workspaces`.
    /// Observable from `package.json` — not a resolver concept.
    pub root_name: String,
}

impl LockWriter for BunLockWriter {
    fn file_name(&self) -> &'static str {
        "bun.lock"
    }

    fn render(&self, entries: &[LockEntry]) -> Result<String> {
        // BTreeMap, so the output is a function of the tree and not of the
        // order a resolver happened to visit it in.
        let mut packages: BTreeMap<&str, Value> = BTreeMap::new();
        for entry in entries {
            if entry.path.is_empty() {
                bail!("a lock entry has an empty install path");
            }
            let value = match &entry.source {
                PackageSource::Registry { name, version, integrity } => json!([
                    format!("{name}@{version}"),
                    // The registry slot: empty means the default one, which
                    // is the only one this installer fetches from.
                    "",
                    // See the module header: intentionally empty.
                    Map::new(),
                    integrity,
                ]),
                PackageSource::Link { target_rel } => json!([format!(
                    "{}@file:{}",
                    // A link's locator names the package, and the install
                    // path's last segment is that name — the path may nest it
                    // under a requester, the locator never does.
                    leaf_name(&entry.path),
                    target_rel.display()
                )]),
            };
            if packages.insert(&entry.path, value).is_some() {
                bail!("two lock entries claim the install path {:?}", entry.path);
            }
        }

        let doc = json!({
            "lockfileVersion": 1,
            // bun records every workspace here with its declared ranges. This
            // writes the root's identity only: ranges are a resolver input,
            // and the reader ignores this section entirely.
            "workspaces": { "": { "name": self.root_name } },
            "packages": packages,
        });
        Ok(format!("{}\n", serde_json::to_string_pretty(&doc)?))
    }
}

/// The package name a bun.lock key installs: everything after the last
/// nesting boundary, scope included. `@scope/pkg/react` → `react`,
/// `@scope/pkg` → `@scope/pkg`.
fn leaf_name(path: &str) -> &str {
    let segs: Vec<&str> = path.split('/').collect();
    match segs.len() {
        0 | 1 => path,
        n => {
            if segs[n - 2].starts_with('@') {
                &path[path.len() - (segs[n - 2].len() + 1 + segs[n - 1].len())..]
            } else {
                segs[n - 1]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::parse_bun_lock;

    fn registry(name: &str, version: &str, integrity: &str) -> PackageSource {
        PackageSource::Registry {
            name: name.to_string(),
            version: version.to_string(),
            integrity: integrity.to_string(),
        }
    }

    fn a_tree() -> Vec<LockEntry> {
        vec![
            LockEntry {
                path: "react".into(),
                source: registry("react", "18.3.1", "sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=="),
            },
            LockEntry {
                path: "@dnd-kit/core".into(),
                source: registry(
                    "@dnd-kit/core",
                    "6.3.1",
                    "sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb==",
                ),
            },
            // The peer instance: a second copy of react nested under the
            // package that needs a different one. This is the entry the
            // format has to express without changing.
            LockEntry {
                path: "@dnd-kit/core/react".into(),
                source: registry("react", "17.0.2", "sha512-cccccccccccccccccccccccccccccccccccccccc=="),
            },
            LockEntry {
                path: "@e2e/shared".into(),
                source: PackageSource::Link { target_rel: PathBuf::from("packages/shared") },
            },
        ]
    }

    /// R771-T5's verify line: a tree containing a nested (peer-instanced)
    /// package survives write → read with nothing lost.
    #[test]
    fn a_tree_with_a_peer_instance_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = a_tree();
        let path = write_lock(
            &BunLockWriter { root_name: "demo".into() },
            tmp.path(),
            &entries,
        )
        .unwrap();
        assert_eq!(path, tmp.path().join("bun.lock"));

        let read = parse_bun_lock(&path).unwrap();
        assert_eq!(read.len(), entries.len());
        for entry in &entries {
            let got = read
                .iter()
                .find(|p| p.key == entry.path)
                .unwrap_or_else(|| panic!("{:?} did not survive the round trip", entry.path));
            assert_eq!(got.source, entry.source, "{:?}", entry.path);
        }

        // ...and the nested entry lands where a nested entry has to land.
        let nested = read.iter().find(|p| p.key == "@dnd-kit/core/react").unwrap();
        assert_eq!(
            nested.dest_rel,
            Path::new("node_modules/@dnd-kit/core/node_modules/react")
        );
        // The hoisted copy is a different version at a different path, which
        // is the whole point of the nesting.
        let hoisted = read.iter().find(|p| p.key == "react").unwrap();
        assert_eq!(hoisted.dest_rel, Path::new("node_modules/react"));
    }

    #[test]
    fn rendering_is_deterministic_and_order_independent() {
        let writer = BunLockWriter { root_name: "demo".into() };
        let forward = writer.render(&a_tree()).unwrap();
        let mut reversed = a_tree();
        reversed.reverse();
        assert_eq!(writer.render(&reversed).unwrap(), forward);
        assert_eq!(writer.render(&a_tree()).unwrap(), forward);
        // Strict JSON, so a stricter reader than ours can consume it too.
        serde_json::from_str::<Value>(&forward).unwrap();
    }

    #[test]
    fn a_duplicated_install_path_is_refused() {
        let mut entries = a_tree();
        entries.push(entries[0].clone());
        let err = BunLockWriter { root_name: "demo".into() }
            .render(&entries)
            .unwrap_err()
            .to_string();
        assert!(err.contains("react"), "{err}");
    }

    /// Print a lock for a real, installable closure — the seam's output as
    /// bytes, so it can be fed to the installer for real:
    ///
    /// ```text
    /// cargo test -p mesofact-build --lib lock::tests::renders_an_installable_lock \
    ///   -- --ignored --nocapture > /tmp/p/bun.lock   # then trim the test harness lines
    /// cargo run -p mesofact-build -- install /tmp/p
    /// ```
    ///
    /// Ignored because it is a fixture generator, not an assertion. The
    /// integrities are real (react 18.3.1 and its closure), so what it emits
    /// installs.
    #[test]
    #[ignore = "fixture generator: prints an installable bun.lock"]
    fn renders_an_installable_lock() {
        let entries = vec![
            LockEntry {
                path: "react".into(),
                source: registry("react", "18.3.1", "sha512-wS+hAgJShR0KhEvPJArfuPVN1+Hz1t0Y6n5jLrGQbkb4urgPE/0Rve+1kMB1v/oWgHgm4WIcV+i7F2pTVj+2iQ=="),
            },
            LockEntry {
                path: "loose-envify".into(),
                source: registry("loose-envify", "1.4.0", "sha512-lyuxPGr/Wfhrlem2CL/UcnUc1zcqKAImBDzukY7Y5F/yQiNdko6+fRLevlw1HgMySw7f611UIY408EtxRSoK3Q=="),
            },
            // Nested on purpose: this is the peer-instance shape, installed
            // for real rather than only round-tripped.
            LockEntry {
                path: "loose-envify/js-tokens".into(),
                source: registry("js-tokens", "4.0.0", "sha512-RdJUflcE3cUzKiMqQgsCu06FPu9UdIJO0beYbPhHN4k6apgJtifcoCtT9bcxOpYBtpD2kCM6Sbzg4CausW/PKQ=="),
            },
        ];
        print!("{}", BunLockWriter { root_name: "seam-fixture".into() }.render(&entries).unwrap());
    }

    #[test]
    fn a_locator_names_the_package_not_the_path() {
        assert_eq!(leaf_name("react"), "react");
        assert_eq!(leaf_name("@dnd-kit/core"), "@dnd-kit/core");
        assert_eq!(leaf_name("@dnd-kit/core/react"), "react");
        assert_eq!(leaf_name("@dnd-kit/core/@types/react"), "@types/react");
        assert_eq!(leaf_name("chalk/ansi-styles"), "ansi-styles");
    }
}
