//! npm's `os` / `cpu` / `libc` manifest gates (R773-F6).
//!
//! The grammar is npm's — a plain entry is an allowlist member, a `!`-prefixed
//! one is a denylist member, an exclusion beats an inclusion, an empty list
//! gates nothing — so it belongs to the crate that models npm manifests rather
//! than to whichever consumer noticed it first.
//!
//! It *was* the other way round: `mesofact-build`'s `install.rs` grew the
//! matcher for `bun.lock` entries and `pnpm.rs` borrowed it. Since R773-T5 gave
//! `mesofact-build` a dependency on this crate the edge points the right way,
//! and both of those now delegate here. Do not write a second `!`-negation
//! matcher anywhere; the two prior copies had already drifted (one knew nothing
//! of `sunos` or `loong64`) which is what a second copy always ends in.

use crate::packument::VersionManifest;

/// The host, in npm's spelling, with each axis absent when this build cannot
/// state it honestly.
///
/// `None` on an axis means *admit everything* at every call site — a gate we
/// cannot evaluate must not be allowed to empty an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    /// Node's `process.platform`: `darwin`, `linux`, `win32`, …
    pub os: Option<&'static str>,
    /// Node's `process.arch`: `arm64`, `x64`, …
    pub cpu: Option<&'static str>,
    /// `glibc` or `musl`. Only ever `Some` on Linux — every other platform
    /// has no npm-modelled libc, and a package gating on one there is gating
    /// on something that cannot be true.
    pub libc: Option<&'static str>,
}

impl Host {
    pub fn current() -> Self {
        let (os, cpu) = match host_platform_arch() {
            Some((os, cpu)) => (Some(os), Some(cpu)),
            None => (None, None),
        };
        Self { os, cpu, libc: host_libc() }
    }

    /// How the host reads in an error message: `darwin/arm64`, or
    /// `linux/x64 (musl)` where a libc is known.
    pub fn describe(&self) -> String {
        let os = self.os.unwrap_or("unknown");
        let cpu = self.cpu.unwrap_or("unknown");
        match self.libc {
            Some(libc) => format!("{os}/{cpu} ({libc})"),
            None => format!("{os}/{cpu}"),
        }
    }
}

/// Why a manifest's platform gates exclude a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformMismatch {
    /// `os`, `cpu` or `libc` — which of the three gates refused.
    pub field: &'static str,
    /// The host value that gate was evaluated against.
    pub host: String,
    /// The manifest's list, verbatim, negations included.
    pub declared: Vec<String>,
}

/// The first of `os` / `cpu` / `libc` that excludes `host`, if any.
///
/// Checked in that order so the message names the most specific reason a human
/// would look for first. An axis the host cannot state admits everything, and
/// an empty list is not a gate — so this returns `None` for every ordinary
/// package, which is the overwhelmingly common case.
pub fn manifest_mismatch(manifest: &VersionManifest, host: &Host) -> Option<PlatformMismatch> {
    gate_mismatch(&manifest.os, &manifest.cpu, &manifest.libc, host)
}

/// [`manifest_mismatch`] over three already-extracted lists.
///
/// The consumers disagree about where the lists come from — a packument's
/// `VersionManifest` here, a `bun.lock` entry's meta object in
/// `mesofact-build`'s `install.rs` — and agree about everything that follows:
/// which axes exist, what order they are checked in, and what "excludes" means.
/// R773-F9 gave the lock reader the same question to answer (a required
/// platform mismatch is now its `EBADPLATFORM` to raise), so it asks it here
/// rather than growing the second copy R773-F6 spent a ticket removing.
pub fn gate_mismatch(
    os: &[String],
    cpu: &[String],
    libc: &[String],
    host: &Host,
) -> Option<PlatformMismatch> {
    for (field, declared, host_value) in
        [("os", os, host.os), ("cpu", cpu, host.cpu), ("libc", libc, host.libc)]
    {
        let Some(host_value) = host_value else { continue };
        if declared.is_empty() {
            continue;
        }
        if !platform_admits(declared.iter().map(String::as_str), host_value) {
            return Some(PlatformMismatch {
                field,
                host: host_value.to_string(),
                declared: declared.to_vec(),
            });
        }
    }
    None
}

/// One `os`/`cpu`/`libc` list against one host value: a plain entry is an
/// allowlist member, a `!`-prefixed one is a denylist member, an exclusion
/// beats an inclusion, and a list with no plain entries gates nothing.
///
/// Takes already-extracted strings because the callers disagree about how the
/// list is *spelled* — a JSON field in `bun.lock`, a YAML sequence in a pnpm
/// lock, a `Vec<String>` on a packument — and never about what it means.
pub fn platform_admits<'a>(values: impl IntoIterator<Item = &'a str>, host: &str) -> bool {
    let mut required = false;
    let mut satisfied = false;
    for v in values {
        match v.strip_prefix('!') {
            Some(excluded) if excluded == host => return false,
            Some(_) => {}
            None => {
                required = true;
                satisfied |= v == host;
            }
        }
    }
    !required || satisfied
}

/// This build's `os`/`cpu` in npm's spelling, or `None` on a target npm does
/// not model.
pub fn host_platform_arch() -> Option<(&'static str, &'static str)> {
    node_platform_arch(std::env::consts::OS, std::env::consts::ARCH)
}

/// Rust's `OS`/`ARCH` in Node's spelling. Split out from
/// [`host_platform_arch`] so the mapping is testable off a host triple rather
/// than only against whatever machine the tests happen to run on.
pub fn node_platform_arch(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    let platform = match os {
        "macos" => "darwin",
        "windows" => "win32",
        // Node reports both Solaris and illumos as "sunos".
        "solaris" | "illumos" => "sunos",
        "linux" => "linux",
        "freebsd" => "freebsd",
        "netbsd" => "netbsd",
        "openbsd" => "openbsd",
        "aix" => "aix",
        _ => return None,
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "arm" => "arm",
        "loongarch64" => "loong64",
        "powerpc64" => "ppc64",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        // Deliberately absent: `mips64`. npm publishes only the
        // little-endian `mips64el` build and Rust spells both `mips64`, so
        // there is no honest mapping here.
        _ => return None,
    };
    // Not every product of these two lists exists upstream. That needs no
    // filtering — a name with no package behind it simply resolves to nothing.
    Some((platform, arch))
}

/// This build's libc in npm's spelling.
///
/// Read off `target_env` at compile time rather than probed at runtime, which
/// is sound in the one direction that matters: a `gnu` binary cannot run on a
/// musl-only system and vice versa, so the target this was built for *is* the
/// libc it will run against. Anything that is not a Linux gnu/musl target
/// answers `None` — `linux-android` and the BSDs have no npm-modelled libc,
/// and guessing one would skip a package the project needs.
pub fn host_libc() -> Option<&'static str> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        Some("glibc")
    }
    #[cfg(all(target_os = "linux", target_env = "musl"))]
    {
        Some("musl")
    }
    #[cfg(not(all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packument::Dist;

    fn manifest(os: &[&str], cpu: &[&str], libc: &[&str]) -> VersionManifest {
        VersionManifest {
            version: "1.0.0".into(),
            dependencies: Default::default(),
            dev_dependencies: Default::default(),
            peer_dependencies: Default::default(),
            peer_dependencies_meta: Default::default(),
            optional_dependencies: Default::default(),
            os: os.iter().map(|s| (*s).to_string()).collect(),
            cpu: cpu.iter().map(|s| (*s).to_string()).collect(),
            libc: libc.iter().map(|s| (*s).to_string()).collect(),
            deprecated: None,
            dist: Dist { tarball: "https://example.test/t.tgz".into(), integrity: None },
        }
    }

    const HOST: Host = Host { os: Some("linux"), cpu: Some("x64"), libc: Some("glibc") };

    #[test]
    fn the_list_grammar_is_allowlist_denylist_and_exclusion_wins() {
        assert!(platform_admits(["linux"], "linux"));
        assert!(!platform_admits(["linux"], "darwin"));
        // A denylist alone admits everything it does not name.
        assert!(platform_admits(["!win32"], "linux"));
        assert!(!platform_admits(["!win32"], "win32"));
        // An exclusion beats an inclusion of the same host.
        assert!(!platform_admits(["linux", "!linux"], "linux"));
        // An empty list is not a gate.
        assert!(platform_admits([], "linux"));
        // bun's literal `"none"` for a platform it does not model matches
        // nothing, which is the intent.
        assert!(!platform_admits(["none"], "linux"));
    }

    #[test]
    fn an_ordinary_manifest_declares_no_gates_and_is_admitted() {
        assert_eq!(manifest_mismatch(&manifest(&[], &[], &[]), &HOST), None);
    }

    #[test]
    fn each_axis_refuses_by_name_and_carries_the_declared_list() {
        let bad = manifest_mismatch(&manifest(&["darwin"], &[], &[]), &HOST)
            .expect("os excludes linux");
        assert_eq!(bad.field, "os");
        assert_eq!(bad.host, "linux");
        assert_eq!(bad.declared, ["darwin"]);

        let m = manifest(&["linux"], &["arm64"], &[]);
        assert_eq!(manifest_mismatch(&m, &HOST).expect("cpu excludes x64").field, "cpu");

        let m = manifest(&["linux"], &["x64"], &["musl"]);
        assert_eq!(manifest_mismatch(&m, &HOST).expect("libc excludes glibc").field, "libc");

        // os is reported before cpu even when both refuse.
        let m = manifest(&["darwin"], &["arm64"], &[]);
        assert_eq!(manifest_mismatch(&m, &HOST).expect("both refuse").field, "os");
    }

    #[test]
    fn an_axis_the_host_cannot_state_gates_nothing() {
        let unknown = Host { os: None, cpu: None, libc: None };
        let m = manifest(&["darwin"], &["arm64"], &["musl"]);
        assert_eq!(manifest_mismatch(&m, &unknown), None);

        // The realistic case: a macOS host has no npm-modelled libc, so a
        // `libc` gate there is unevaluable rather than failing.
        let darwin = Host { os: Some("darwin"), cpu: Some("arm64"), libc: None };
        let m = manifest(&["darwin"], &["arm64"], &["glibc"]);
        assert_eq!(manifest_mismatch(&m, &darwin), None);
    }

    #[test]
    fn the_node_spelling_table_covers_the_hosts_npm_publishes_for() {
        assert_eq!(node_platform_arch("macos", "aarch64"), Some(("darwin", "arm64")));
        assert_eq!(node_platform_arch("linux", "x86_64"), Some(("linux", "x64")));
        assert_eq!(node_platform_arch("windows", "aarch64"), Some(("win32", "arm64")));
        assert_eq!(node_platform_arch("illumos", "x86_64"), Some(("sunos", "x64")));
        assert_eq!(node_platform_arch("linux", "loongarch64"), Some(("linux", "loong64")));
        assert_eq!(node_platform_arch("android", "aarch64"), None);
        assert_eq!(node_platform_arch("linux", "mips64"), None);
    }

    #[test]
    fn the_running_host_is_describable_and_its_libc_agrees_with_its_os() {
        let host = Host::current();
        assert!(!host.describe().is_empty());
        if host.libc.is_some() {
            assert_eq!(host.os, Some("linux"), "only Linux has an npm-modelled libc");
        }
    }
}
