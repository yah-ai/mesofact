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
//! `Registry{name, version, integrity, platform, optional} | Link{target}`
//! vocabulary the three parsers already reduce their formats to. The last two
//! joined in R773-F9 and belong under this rule rather than beside it: "can
//! this run here" and "may it be missing" are both questions the materializer
//! asks and answers. No ranges, no peer relationships, no resolution
//! provenance. Those are real things a resolver knows, and a
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
//! # What goes in the third slot
//!
//! bun's own writer puts a package's declared `dependencies`,
//! `peerDependencies`, `bin`, `os` and `cpu` in the third slot of each entry.
//!
//! This used to write `{}` there and say so: a materializer never read it, and
//! the rule above says the seam does not carry what the materializer cannot
//! observe. **R773-F9 reversed that**, because the materializer now *does* read
//! it. The lock is portable — the resolver records every platform's build of a
//! native package instead of only the one the resolving machine could run — so
//! each entry has to state what it runs on or an installer has no way to pick.
//! Three keys are written, and only for a package that declares them:
//!
//! - `os` / `cpu`, in bun's own spelling: a bare string for a single value, an
//!   array for several (verified against `bun install --lockfile-only` 1.3.12
//!   and the recorded `optional-platform-gated` corpus case, which carries all
//!   twenty-four `@esbuild/*` variants).
//! - `libc`, which **bun does not record at all** — checked on a real
//!   `sharp@0.33.5` lock, where the `linuxmusl` builds are written with `os`
//!   and `cpu` only. npm's manifest field is the spelling; we write it because
//!   dropping it would make a musl and a glibc build of the same package
//!   indistinguishable at install time.
//! - `optional`, an explicit boolean on every registry entry, and **not a bun
//!   concept**: bun records optionality on the *requester*, as an
//!   `optionalDependencies` map. A reader needs it per entry to know whether a
//!   platform mismatch is a skip or an `EBADPLATFORM` error, and reconstructing
//!   it from requester maps would mean carrying the whole dependency graph the
//!   rule above keeps out. `install.rs` treats an entry that does not state it
//!   as optional, so a bun-authored lock keeps working unchanged.
//!
//! bun tolerates all three: `bun install --frozen-lockfile --dry-run` accepts a
//! lock carrying `libc` and `optional` keys (bun 1.3.12, checked 2026-09-03).
//! What it still will not do is *trust* one of our locks' graph — the
//! `dependencies` / `peerDependencies` / `bin` keys are still absent, so the
//! `bun` CLI would re-resolve rather than install from it. Our reader ignores
//! those keys entirely.

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::install::{PackageSource, PlatformGates};

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

/// What [`BunLockWriter`] writes, as a name a caller can join onto a project
/// root without constructing a writer to ask.
pub const BUN_LOCK_FILE: &str = "bun.lock";

impl LockWriter for BunLockWriter {
    fn file_name(&self) -> &'static str {
        BUN_LOCK_FILE
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
                PackageSource::Registry { name, version, integrity, platform, optional } => {
                    json!([
                        format!("{name}@{version}"),
                        // The registry slot: empty means the default one, which
                        // is the only one this installer fetches from.
                        "",
                        entry_meta(platform, *optional),
                        integrity,
                    ])
                }
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

/// The third slot of a registry entry: what an installer needs to decide
/// whether this package belongs on the machine in front of it.
///
/// `os` / `cpu` / `libc` are omitted where the package declares none, which is
/// the overwhelming majority — so an ordinary lock looks exactly as it did
/// before R773-F9. `optional` is written whenever it is known — which is always
/// for a resolver-produced entry, and never for one read back out of a
/// bun-authored lock, since bun does not record it. The reader treats a missing
/// one as "the lock does not say" and skips rather than refusing, so this
/// omission round-trips as itself. See the module header.
fn entry_meta(platform: &PlatformGates, optional: Option<bool>) -> Value {
    let mut meta = Map::new();
    for (field, values) in
        [("os", &platform.os), ("cpu", &platform.cpu), ("libc", &platform.libc)]
    {
        match values.as_slice() {
            [] => {}
            // bun's own spelling: one value is a bare string, several are an
            // array. Both are npm's grammar and both read back identically.
            [only] => {
                meta.insert(field.to_string(), Value::String(only.clone()));
            }
            many => {
                meta.insert(field.to_string(), json!(many));
            }
        }
    }
    if let Some(optional) = optional {
        meta.insert("optional".to_string(), Value::Bool(optional));
    }
    Value::Object(meta)
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
    use crate::install::{parse_bun_lock, parse_bun_lock_for_host};

    fn registry(name: &str, version: &str, integrity: &str) -> PackageSource {
        PackageSource::Registry {
            name: name.to_string(),
            version: version.to_string(),
            integrity: integrity.to_string(),
            platform: PlatformGates::default(),
            optional: Some(false),
        }
    }

    /// A native binary: the shape the portable lock exists for. `gates` are
    /// npm's lists verbatim, and `optional` is what makes a mismatch a skip
    /// rather than an error on the reading side.
    fn native(
        name: &str,
        version: &str,
        integrity: &str,
        gates: PlatformGates,
        optional: bool,
    ) -> PackageSource {
        PackageSource::Registry {
            name: name.to_string(),
            version: version.to_string(),
            integrity: integrity.to_string(),
            platform: gates,
            optional: Some(optional),
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

    /// The portable half of the lock (R773-F9): every platform's build is
    /// written, each stating what it runs on, and the entry survives write →
    /// read with its gates and its optionality intact.
    ///
    /// Read back against a host that admits everything, because a host-filtered
    /// read is *supposed* to lose most of these — that filtering is the point,
    /// and it is asserted separately in `install.rs`.
    #[test]
    fn platform_gates_and_optionality_survive_the_round_trip() {
        let gates = |os: &[&str], cpu: &[&str], libc: &[&str]| PlatformGates {
            os: os.iter().map(|s| s.to_string()).collect(),
            cpu: cpu.iter().map(|s| s.to_string()).collect(),
            libc: libc.iter().map(|s| s.to_string()).collect(),
        };
        let entries = vec![
            LockEntry {
                path: "@esbuild/darwin-arm64".into(),
                source: native(
                    "@esbuild/darwin-arm64",
                    "0.24.0",
                    "sha512-dddddddddddddddddddddddddddddddddddddddd==",
                    gates(&["darwin"], &["arm64"], &[]),
                    true,
                ),
            },
            LockEntry {
                path: "@img/sharp-linuxmusl-x64".into(),
                source: native(
                    "@img/sharp-linuxmusl-x64",
                    "0.33.5",
                    "sha512-mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm==",
                    gates(&["linux"], &["x64"], &["musl"]),
                    true,
                ),
            },
            // Several values on one axis, and a negation: npm's array form,
            // which bun collapses to a bare string only when there is one.
            LockEntry {
                path: "posix-only".into(),
                source: native(
                    "posix-only",
                    "1.0.0",
                    "sha512-pppppppppppppppppppppppppppppppppppppppp==",
                    gates(&["!win32"], &["x64", "arm64"], &[]),
                    false,
                ),
            },
            LockEntry {
                path: "react".into(),
                source: registry("react", "18.3.1", "sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=="),
            },
        ];

        let tmp = tempfile::tempdir().unwrap();
        let path =
            write_lock(&BunLockWriter { root_name: "demo".into() }, tmp.path(), &entries).unwrap();

        // bun's own spelling, checked against a real `bun install` lock: one
        // value is a bare string, several are an array.
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            doc["packages"]["@esbuild/darwin-arm64"][2],
            json!({ "os": "darwin", "cpu": "arm64", "optional": true })
        );
        assert_eq!(
            doc["packages"]["posix-only"][2],
            json!({ "os": "!win32", "cpu": ["x64", "arm64"], "optional": false })
        );
        // An ordinary package declares no gates, so it grows no keys but the
        // one this format does not have at all.
        assert_eq!(doc["packages"]["react"][2], json!({ "optional": false }));

        let admit_everything = rnpm::Host { os: None, cpu: None, libc: None };
        let read = parse_bun_lock_for_host(&path, &admit_everything).unwrap();
        assert_eq!(read.len(), entries.len());
        for entry in &entries {
            let got = read
                .iter()
                .find(|p| p.key == entry.path)
                .unwrap_or_else(|| panic!("{:?} did not survive the round trip", entry.path));
            assert_eq!(got.source, entry.source, "{:?}", entry.path);
        }
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
