//! Materialize a store entry into a project (R771-F2, W319 §3).
//!
//! A project's `node_modules` is a *view* of the content-addressed
//! [`crate::store`], not a copy of a tarball. Each file is placed with the
//! first of three strategies that works, in this order — which is not pnpm's:
//!
//! 1. **reflink** (`reflink-copy`) — copy-on-write. Cheap *and* safe: a
//!    consumer that edits a file under `node_modules` gets its own copy and
//!    cannot corrupt the store. APFS, btrfs, xfs, ReFS.
//! 2. **hardlink** — cheap, but the inode is shared, so a write into one
//!    project's `node_modules` silently rewrites the store entry for every
//!    project on the machine. pnpm accepts that; here it is only the
//!    fallback.
//! 3. **copy** — always correct, always available; what a store and a project
//!    on different filesystems get.
//!
//! **The chain is tried per file, and the result is deliberately not cached
//! across a tree.** Caching would be faster — the filesystem pair is fixed
//! for the whole call — but a single odd failure would then demote every
//! remaining file to hardlinks, which is the one rung with a safety cost.
//! Paying one failed syscall per file to never make that trade silently is
//! the right side of that bargain.
//!
//! **Windows is out of scope** (W319 §3) and fails loudly rather than
//! half-working through junctions; see [`materialize_tree`].

use anyhow::{bail, Context, Result};
use std::path::Path;

/// How one file was placed. Also the knob: a caller can restrict the chain to
/// force a lower rung, which is how the copy fallback gets tested without a
/// second filesystem.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Strategy {
    Reflink,
    Hardlink,
    Copy,
}

impl Strategy {
    fn name(self) -> &'static str {
        match self {
            Strategy::Reflink => "reflink",
            Strategy::Hardlink => "hardlink",
            Strategy::Copy => "copy",
        }
    }
}

/// The full chain, most-preferred first.
pub const FULL_CHAIN: &[Strategy] = &[Strategy::Reflink, Strategy::Hardlink, Strategy::Copy];

/// Points the chain at a lower rung — for CI or a bug report that needs the
/// store and the project to behave as if they were on different filesystems.
pub const CHAIN_ENV: &str = "MESOFACT_MATERIALIZE";

/// The chain named by `$MESOFACT_MATERIALIZE` (`reflink`, `hardlink`, `copy`),
/// else the full one. Naming a rung *starts* the chain there rather than
/// pinning it: `hardlink` still falls back to `copy`, so the override can only
/// ever remove optimizations, never correctness.
pub fn chain_from_env() -> Result<&'static [Strategy]> {
    let Some(raw) = std::env::var_os(CHAIN_ENV) else {
        return Ok(FULL_CHAIN);
    };
    match raw.to_str() {
        None | Some("") | Some("auto") => Ok(FULL_CHAIN),
        Some("reflink") => Ok(FULL_CHAIN),
        Some("hardlink") => Ok(&FULL_CHAIN[1..]),
        Some("copy") => Ok(&FULL_CHAIN[2..]),
        Some(other) => bail!(
            "{CHAIN_ENV}={other:?} is not a materialization strategy — expected auto, reflink, hardlink or copy"
        ),
    }
}

/// Recreate the tree at `src` under `dest`, placing every file with
/// [`materialize_file`].
///
/// `dest` is created; anything already there is left alone, so the caller
/// clears a stale destination first (`install_registry_package` does).
pub fn materialize_tree(src: &Path, dest: &Path, chain: &[Strategy]) -> Result<()> {
    #[cfg(not(unix))]
    bail!(
        "materializing {} is unix-only: Windows is out of scope for the Rust-native installer (W319 §3), and half-supporting it through junctions would be worse than not supporting it",
        dest.display()
    );
    #[cfg(unix)]
    {
        std::fs::create_dir_all(dest)
            .with_context(|| format!("creating {}", dest.display()))?;
        for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
            let entry = entry?;
            let from = entry.path();
            let to = dest.join(entry.file_name());
            let kind = entry.file_type()?;
            if kind.is_dir() {
                materialize_tree(&from, &to, chain)?;
            } else if kind.is_symlink() {
                // Neither reflink nor hardlink says anything useful about a
                // symlink; the link itself is the content, so recreate it.
                let target = std::fs::read_link(&from)?;
                std::os::unix::fs::symlink(&target, &to)
                    .with_context(|| format!("symlinking {}", to.display()))?;
            } else {
                materialize_file(&from, &to, chain)?;
            }
        }
        Ok(())
    }
}

/// Place one file, returning the rung that worked.
///
/// Every rung preserves the mode: `clonefile`/`FICLONE` copy the metadata,
/// `std::fs::copy` copies the permissions, and a hardlink *is* the same inode.
/// So an executable in a package's `bin/` stays executable however it lands.
pub fn materialize_file(src: &Path, dest: &Path, chain: &[Strategy]) -> Result<Strategy> {
    let mut last: Option<(Strategy, std::io::Error)> = None;
    for &strategy in chain {
        let attempt = match strategy {
            Strategy::Reflink => reflink_copy::reflink(src, dest),
            Strategy::Hardlink => std::fs::hard_link(src, dest),
            Strategy::Copy => std::fs::copy(src, dest).map(|_| ()),
        };
        match attempt {
            Ok(()) => return Ok(strategy),
            Err(e) => {
                // A rung that failed may still have left a destination behind
                // (a partial copy); the next rung needs a clear path.
                let _ = std::fs::remove_file(dest);
                last = Some((strategy, e));
            }
        }
    }
    match last {
        // The chain always ends in `copy`, so reaching here means the file
        // genuinely could not be written — report the last rung's error
        // rather than a summary, since that one is the real cause.
        Some((strategy, e)) => Err(e).with_context(|| {
            format!("{} {} to {}", strategy.name(), src.display(), dest.display())
        }),
        None => bail!("empty materialization chain for {}", dest.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// A store-entry-shaped tree: nested dirs, an executable, a symlink.
    fn an_entry() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("entry");
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("package.json"), r#"{"name":"demo"}"#).unwrap();
        std::fs::write(src.join("lib/index.js"), "module.exports = 1;\n").unwrap();
        std::fs::write(src.join("bin/demo"), "#!/usr/bin/env node\n").unwrap();
        std::fs::set_permissions(src.join("bin/demo"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::os::unix::fs::symlink("../lib/index.js", src.join("bin/linked.js")).unwrap();
        (tmp, src)
    }

    fn assert_tree_matches(src: &Path, dest: &Path) {
        assert_eq!(
            std::fs::read_to_string(dest.join("package.json")).unwrap(),
            std::fs::read_to_string(src.join("package.json")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("lib/index.js")).unwrap(),
            "module.exports = 1;\n"
        );
        assert_eq!(
            std::fs::metadata(dest.join("bin/demo")).unwrap().permissions().mode() & 0o777,
            0o755,
            "the executable bit did not survive materialization"
        );
        assert_eq!(
            std::fs::read_link(dest.join("bin/linked.js")).unwrap(),
            Path::new("../lib/index.js")
        );
    }

    /// The three rungs are interchangeable as far as the resulting tree is
    /// concerned — which is the whole reason a fallback chain is allowed to
    /// exist. `copy` is W319 §3's cross-filesystem case, forced here without
    /// needing a second filesystem.
    #[test]
    fn every_rung_produces_the_same_tree() {
        let (tmp, src) = an_entry();
        for chain in [FULL_CHAIN, &FULL_CHAIN[1..], &FULL_CHAIN[2..]] {
            let dest = tmp.path().join(format!("out-{}", chain.len()));
            materialize_tree(&src, &dest, chain).unwrap();
            assert_tree_matches(&src, &dest);
        }
    }

    /// The safety property the order exists for: only the hardlink rung
    /// shares the inode, and a consumer editing `node_modules` must not reach
    /// back into the store through the other two.
    #[test]
    fn only_the_hardlink_rung_writes_through_to_the_store() {
        let (tmp, src) = an_entry();
        let dest = tmp.path().join("out");
        let file = "lib/index.js";

        let strategy = {
            std::fs::create_dir_all(dest.join("lib")).unwrap();
            materialize_file(&src.join(file), &dest.join(file), FULL_CHAIN).unwrap()
        };
        std::fs::write(dest.join(file), "edited by a consumer\n").unwrap();
        let store_now = std::fs::read_to_string(src.join(file)).unwrap();

        match strategy {
            Strategy::Reflink | Strategy::Copy => {
                assert_eq!(store_now, "module.exports = 1;\n", "{strategy:?} corrupted the store");
            }
            Strategy::Hardlink => {
                // Documented, accepted, and the reason this rung is second.
                assert_eq!(store_now, "edited by a consumer\n");
            }
        }
    }

    /// Not asserted as a *requirement* — a filesystem without CoW is a valid
    /// place to build — but on this camp's dev platform (APFS) the top rung
    /// is expected to win, and a silent demotion to hardlinks is exactly the
    /// regression worth catching.
    #[cfg(target_os = "macos")]
    #[test]
    fn reflink_is_what_wins_on_apfs() {
        let (tmp, src) = an_entry();
        let dest = tmp.path().join("out.js");
        assert_eq!(
            materialize_file(&src.join("lib/index.js"), &dest, FULL_CHAIN).unwrap(),
            Strategy::Reflink
        );
    }

    #[test]
    fn a_restricted_chain_starts_at_the_named_rung() {
        assert_eq!(FULL_CHAIN, &[Strategy::Reflink, Strategy::Hardlink, Strategy::Copy]);
        assert_eq!(&FULL_CHAIN[1..], &[Strategy::Hardlink, Strategy::Copy]);
        assert_eq!(&FULL_CHAIN[2..], &[Strategy::Copy]);
    }

    #[test]
    fn a_missing_source_fails_naming_the_last_rung() {
        let tmp = tempfile::tempdir().unwrap();
        let err = format!(
            "{:#}",
            materialize_file(
                &tmp.path().join("absent"),
                &tmp.path().join("out"),
                FULL_CHAIN
            )
            .unwrap_err()
        );
        assert!(err.contains("copy"), "{err}");
        assert!(err.contains("absent"), "{err}");
    }
}
