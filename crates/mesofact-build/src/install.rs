//! Minimal npm install step (re-scoped R447). The pipeline carries its own
//! small installer rather than an off-the-shelf one; the R446 audit (15
//! packages, pure JS, zero install scripts) is what justifies the narrow
//! scope below.
//!
//! Two claims that used to sit here were wrong and are corrected by R757-S1
//! (see `.yah/docs/working/W318-npm-resolution-substrate.md`). pacquet was not
//! "retired upstream" — it moved into the pnpm monorepo and, as of pnpm 11.7,
//! is pnpm's opt-in Rust engine doing resolution *and* materialization; the
//! crates.io 0.0.0 entry is a name reservation. Orogene is not "dormant" — it
//! went closed-source with paid licenses, and its public tree no longer
//! contains source. Neither changes the decision below; both change what a
//! future reader should conclude from it.
//!
//! This module owns MATERIALIZATION only. Resolution (semver ranges +
//! packuments -> exact versions) is R757's separate half:
//!
//! - **Lockfile-driven only.** Exact versions + sha512 integrity come from an
//!   existing lockfile; this step never resolves semver ranges. No lock →
//!   error, pointing at `bun install` / `npm install` / `pnpm install` (or a
//!   committed lockfile). Three formats are read (W319 §4): `bun.lock`,
//!   `package-lock.json` (`lockfileVersion` 2 or 3) and `pnpm-lock.yaml` v9,
//!   in that precedence when a project carries several.
//! - **The lock says where — except pnpm's, which doesn't.** bun and npm keys
//!   are install paths, so their layout is read, not computed: a flat closure
//!   stays flat and a conflict copy lands nested under its parent's
//!   `node_modules/` (`dest_for`). `pnpm-lock.yaml` is keyed by package
//!   identity and says nothing about disk, so [`crate::pnpm`] *derives* a
//!   hoisted layout from its graph (R771-F4, W319 §4.1). That derivation is
//!   the one place this crate hoists, and it hoists to reproduce a tree Node
//!   resolves identically to pnpm's — not to re-resolve anything.
//! - **No lifecycle scripts.** install/postinstall are skipped uncondition-
//!   ally (the W174 "skip-by-default" policy; the audit found zero users).
//! - **`file:` deps symlink** to their target, matching bun's behavior for
//!   workspace-style links (`@mesofact/runtime`) and npm's `"link": true`
//!   entries.
//! - **Platform-gated entries are skipped — or refused.** A lock entry whose
//!   `os`/`cpu`/`libc` metadata excludes this host is dropped rather than
//!   fetched: the one deliberate omission the walk makes, and the reason a
//!   `typescript@7` dep costs one 27 MB native compiler instead of twenty.
//!   Since R773-F9 that is only true of an entry the lock marks **optional**;
//!   a *required* entry that cannot run here is `EBADPLATFORM`, an error
//!   naming the package and the host. That refusal used to live in `rnpm`'s
//!   resolver, where it made every lock host-specific — a lock cut on macOS
//!   had no linux binaries in it at all. Resolution is now portable and this
//!   is the layer that knows what it is installing onto.
//! - **Nothing is fetched unverified.** A registry entry with no sha512
//!   integrity in the lock is refused by name, never downloaded on trust.
//!   `PackageSource::Registry` carries the integrity as a required field, so
//!   there is no path through this module that fetches without one.
//! - **Downloaded once per machine, not once per project.** A package is
//!   unpacked into the content-addressed [`crate::store`] under its integrity
//!   (R771-F1, W319 §2) and every project materializes out of that entry with
//!   the reflink → hardlink → copy chain in [`crate::materialize`] (R771-F2,
//!   W319 §3). The old per-project `gunzip`+`untar` of a cached `.tgz` is
//!   gone, and so is its poisoned-cache eviction: nothing durable is written
//!   until it has been verified.
//! - **Unix only, loudly.** Windows is out of scope for stage 1 (W319 §3):
//!   `install` refuses up front rather than half-working through junctions.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha512};
use std::path::{Path, PathBuf};

use rnpm::Host;

use crate::materialize::{self, materialize_tree, Strategy};
use crate::store::Store;

pub(crate) const REGISTRY: &str = "https://registry.npmjs.org";

/// Cache namespace for [`REGISTRY`]'s packuments, and the directory component
/// under a packument cache root.
///
/// It lives next to the URL it names because every caller that resolves against
/// this registry has to agree on it: `registry_id` is the cache key's
/// namespace, so two spellings would read as a total cache miss rather than as
/// a disagreement. Both the conformance corpus and
/// [`crate::project_lock`] take it from here.
pub const REGISTRY_ID: &str = "npm";

/// The endpoint a resolve against the public npm registry runs on.
pub fn registry_endpoint() -> rnpm::RegistryEndpoint {
    rnpm::RegistryEndpoint::new(REGISTRY_ID, REGISTRY)
}

/// A host that admits every platform — the right reader for a lock being read
/// as *data* rather than as an install plan.
///
/// The lock is host-independent (R773-F9): it carries every platform's build of
/// a native package and the *installer* filters. So anything reading a lock to
/// learn what it selected — the conformance recorder's packument prune,
/// [`crate::project_lock`]'s preferences — has to switch the filter off, or it
/// reads a mac's slice of a portable file and mistakes it for the whole.
pub(crate) fn admit_every_platform() -> Host {
    Host { os: None, cpu: None, libc: None }
}

pub struct InstallReport {
    pub installed: usize,
    pub linked: usize,
    pub skipped_fresh: bool,
}

/// Strip JSONC-isms bun.lock carries (trailing commas) with a string-aware
/// scanner — the integrity hashes can contain any base64 byte, so a naive
/// regex is off the table.
fn strip_trailing_commas(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            ',' => {
                // Lookahead: a comma whose next non-whitespace is } or ] is
                // dropped.
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                } else {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// One lockfile entry reduced to what materialization needs: where it goes
/// and where it comes from. This is the whole contract between a parser and
/// the install walk — `parse_bun_lock`, `parse_package_lock` and
/// [`crate::pnpm::parse_pnpm_lock`] all produce it, and the walk below reads
/// nothing else. The first two *read* `dest_rel` (their keys are install
/// paths); the pnpm one *derives* it, which is the entire difficulty of that
/// format and none of this walk's business.
#[derive(Debug)]
pub(crate) struct LockedPackage {
    /// The lockfile key, verbatim. Diagnostics only — never a package name
    /// (it is an install path, or for pnpm a peer-suffixed identity) and
    /// never a URL.
    pub(crate) key: String,
    /// Install path relative to the project root, e.g.
    /// `node_modules/@babel/core/node_modules/convert-source-map`.
    pub(crate) dest_rel: PathBuf,
    pub(crate) source: PackageSource,
}

/// Where a package comes from — the whole vocabulary a materializer has, and
/// therefore (R771-T5) the whole vocabulary the stage-2 lock seam may use.
/// Public for [`crate::lock`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSource {
    /// Fetch `name`@`version` from the public registry. `name` is the
    /// *registry* name, which is not derivable from the key alone — the key
    /// is a path, and an aliased dep (`npm:`) is installed under a directory
    /// name that differs from the package it names.
    ///
    /// `integrity` is required rather than optional: it is both the
    /// verification and the store key (W319 §2), so an entry without one
    /// cannot be fetched *or* addressed. Every parser refuses such an entry
    /// by name, and this type is what keeps that from being a convention.
    Registry {
        name: String,
        version: String,
        integrity: String,
        /// npm's `os`/`cpu`/`libc` for this package, verbatim (R773-F9).
        ///
        /// Carried because a lock this installer *writes* is portable: the
        /// resolver no longer drops a package that cannot run here, so the
        /// entry has to say what it can run on or the filter below would have
        /// nothing to read. Empty on every ordinary package.
        platform: PlatformGates,
        /// Is this package's absence tolerable — was it reached only through
        /// `optionalDependencies`? `None` where the lockfile does not say.
        ///
        /// The other half of R773-F9: a platform-mismatched entry is skipped
        /// when this is `Some(true)` and an `EBADPLATFORM` error when it is
        /// `Some(false)`. npm's refusal, moved from resolve time (where it made
        /// the lock host-specific) to install time (where the host is known).
        ///
        /// Three-valued rather than a `bool` because **bun's format has no slot
        /// for this** — bun records optionality on the *requester*, as an
        /// `optionalDependencies` map inside its meta, so a bun-authored lock
        /// says nothing about any individual entry. Collapsing that silence to
        /// `false` would turn every `@esbuild/*` in a lock `bun install` wrote
        /// into an `EBADPLATFORM` failure on twenty-three of twenty-four hosts.
        /// `None` therefore behaves as "skip", which is what this installer did
        /// before there was a flag at all; only a lock that *states* the entry
        /// is required can produce the error.
        optional: Option<bool>,
    },
    /// Symlink to a path relative to the project root (`file:` deps in
    /// bun.lock, `"link": true` entries in package-lock, `link:` specifiers
    /// in pnpm-lock).
    Link { target_rel: PathBuf },
}

/// A package's declared `os` / `cpu` / `libc`, in npm's grammar (a plain entry
/// allows, a `!`-prefixed one denies, an empty list gates nothing).
///
/// Grouped rather than three fields on [`PackageSource::Registry`] so the
/// parsers that have nothing to say — the npm and pnpm readers, which prune by
/// platform as they parse and so only ever emit entries this host admits —
/// spell that as [`PlatformGates::default`] instead of three empty vectors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformGates {
    pub os: Vec<String>,
    pub cpu: Vec<String>,
    pub libc: Vec<String>,
}

impl PlatformGates {
    /// The first of `os` / `cpu` / `libc` that excludes `host`.
    ///
    /// A delegation, not a matcher: [`rnpm::gate_mismatch`] owns which axes
    /// exist, what order they are checked in and what "excludes" means, and
    /// [`rnpm::manifest_mismatch`] asks it the same question about a packument.
    /// R773-F6 spent a ticket collapsing two drifted copies of that rule into
    /// one; this is the call that keeps it at one.
    fn mismatch(&self, host: &Host) -> Option<rnpm::PlatformMismatch> {
        rnpm::gate_mismatch(&self.os, &self.cpu, &self.libc, host)
    }
}

/// This host's `(process.platform, process.arch)` — npm's spelling, which is
/// not Rust's. The table lives in [`crate::check`], which needed it first to
/// name TypeScript's per-platform native package; sharing it is deliberate,
/// since an `os`/`cpu` lock field and a `@typescript/typescript-<os>-<arch>`
/// directory name are the same vocabulary.
///
/// `None` on a host that table does not model. The gate then admits
/// everything, which is the pre-R832-T4 behaviour — no pruning is a fatter
/// install, but a wrong host name would be an *empty* one.
pub(crate) fn host_platform_arch() -> Option<(&'static str, &'static str)> {
    rnpm::host_platform_arch()
}

/// Does an entry's `os` / `cpu` / `libc` metadata admit this host?
///
/// The npm reader's gate. The bun reader asks the same question through
/// [`PlatformGates::mismatch`], which additionally says *which* axis refused so
/// a required entry can be refused by name; keep the two answering alike.
///
/// **This is the one place the npm walk drops a lock entry on purpose**
/// (R832-T4). The rest of the module installs everything the lock lists —
/// `dev`, `optional` and `peer` alike — because pruning by *intent* is a
/// resolution question. Pruning by *platform* is not: a package declaring
/// `"os": "linux"` cannot execute here whatever the resolver decided, the lock
/// states that itself, and npm/bun/pnpm all skip it. Installing them anyway was
/// a wart this module's header used to admit to; `typescript@7` is what made it
/// unaffordable, since it fans out to twenty native compilers at ~27 MB each
/// and exactly one of them can run.
///
/// Absent fields admit everything, which is the overwhelmingly common case —
/// this returns `true` for every ordinary package.
///
/// The host is a parameter rather than `Host::current()` because a portable
/// lock lists every platform variant of a native (all 24 `@esbuild/*` entries,
/// each with its own `os`/`cpu` meta), so which subset a read yields is
/// entirely a function of the host it is asked about — and two callers want to
/// ask about something other than this machine. R773-F7's conformance corpus
/// and its recorder both read the bun oracle with every gate *open*, so the
/// resolver's portable tree and the oracle can be compared entry for entry.
///
/// `libc` is checked here too, which `host_platform_arch` cannot express: it
/// returns only `(os, cpu)`. [`rnpm::Host`] carries all three and
/// `rnpm::manifest_mismatch` gates on all three, so a bun-side filter that
/// skipped `libc` would drop a `@img/sharp-linux-*` musl/glibc pair back into
/// the diff as a phantom divergence.
fn host_supports(meta: &Value, host: &Host) -> bool {
    for (field, host_value) in [("os", host.os), ("cpu", host.cpu), ("libc", host.libc)] {
        let Some(host_value) = host_value else { continue };
        if !platform_field_admits(meta.get(field), host_value) {
            return false;
        }
    }
    true
}

/// A lock entry's meta object, reduced to its three platform lists.
///
/// npm allows a bare string or an array on each; bun writes the string form.
/// Anything else is metadata this reader does not model, and yields an empty
/// list — which gates nothing, so an unknown shape cannot silently empty an
/// install.
fn platform_gates(meta: &Value) -> PlatformGates {
    let list = |field: &str| match meta.get(field) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    PlatformGates { os: list("os"), cpu: list("cpu"), libc: list("libc") }
}

/// One `os`/`cpu` field against one host value. npm allows a bare string, an
/// array, and `!`-negated entries (`["!win32"]` = anything but Windows); bun
/// writes the string form, npm the array form, and both write the literal
/// `"none"` for a platform they do not model — which matches nothing, as
/// intended.
fn platform_field_admits(field: Option<&Value>, host: &str) -> bool {
    let Some(field) = field else {
        return true;
    };
    let values: Vec<&str> = match field {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        // Anything else is metadata this walk does not understand; admitting
        // it keeps an unknown shape from silently emptying an install.
        _ => return true,
    };
    platform_admits(values, host)
}

/// The `os`/`cpu` list semantics themselves, over already-extracted strings:
/// a plain entry is an allowlist member, a `!`-prefixed one is a denylist
/// member, an exclusion beats an inclusion, and an empty list gates nothing.
///
/// The rule is npm manifest grammar, so it lives in [`rnpm::platform`] and this
/// is a delegation (R773-F6). It stays `pub(crate)` here because
/// [`crate::pnpm`] and the tests below call it by this name; what changed is
/// that there is now one implementation in the crate that models npm manifests
/// rather than a copy in each consumer that noticed it.
pub(crate) fn platform_admits<'a>(values: impl IntoIterator<Item = &'a str>, host: &str) -> bool {
    rnpm::platform_admits(values, host)
}

pub(crate) fn parse_bun_lock(lock_path: &Path) -> Result<Vec<LockedPackage>> {
    parse_bun_lock_for_host(lock_path, &Host::current())
}

/// [`parse_bun_lock`], with the platform gate evaluated against an explicit
/// host instead of this machine.
///
/// Two kinds of caller need the parameter, and neither is an install:
/// [`crate::conformance`] and its recorder read a lock with every gate *open*
/// (an all-`None` host), which is what makes a portable lock and the resolver's
/// portable tree comparable entry for entry; and the tests below state a host
/// so their assertions do not depend on the box running them. An install always
/// goes through [`parse_bun_lock`], i.e. `Host::current()`.
pub(crate) fn parse_bun_lock_for_host(
    lock_path: &Path,
    host: &Host,
) -> Result<Vec<LockedPackage>> {
    let raw = std::fs::read_to_string(lock_path)
        .with_context(|| format!("reading {}", lock_path.display()))?;
    let parsed: Value = serde_json::from_str(&strip_trailing_commas(&raw))
        .with_context(|| format!("parsing {} (after JSONC strip)", lock_path.display()))?;
    let Some(packages) = parsed.get("packages").and_then(Value::as_object) else {
        bail!("{}: no \"packages\" map", lock_path.display());
    };
    let mut out = Vec::new();
    for (key, entry) in packages {
        let Some(arr) = entry.as_array() else {
            bail!("{}: packages[{key}] is not an array", lock_path.display());
        };
        let Some(locator) = arr.first().and_then(Value::as_str) else {
            bail!("{}: packages[{key}] has no locator", lock_path.display());
        };
        // bun.lock entry shapes: [locator, registry?, meta, integrity] for
        // registry packages; [locator, meta] for file:/workspace links.
        let integrity = arr.iter().rev().find_map(Value::as_str).and_then(|s| {
            s.starts_with("sha512-").then(|| s.to_string())
        });
        // Locator forms: "<name>@<version>" | "<name>@file:<path>" — find the
        // @ separating name from source (names may start with @scope/).
        let at = locator
            .rfind('@')
            .filter(|&i| i > 0)
            .ok_or_else(|| anyhow!("unparseable locator {locator:?} for {key}"))?;
        // The *registry* name comes from the locator, never from the lock
        // key: the key is an install path, so a nested entry keyed
        // "@mesofact/runtime/typescript" would otherwise be fetched as a
        // package of that name (a guaranteed 404).
        let name = &locator[..at];
        let version = &locator[at + 1..];
        // The meta object is the only object in the entry array, and it is
        // where bun records `os` / `cpu` — and where we record `libc` and
        // `optional` besides (see [`crate::lock`]).
        let meta = arr.iter().find(|v| v.is_object());
        let platform = meta.map(platform_gates).unwrap_or_default();
        let optional = meta.and_then(|m| m.get("optional")).and_then(Value::as_bool);
        let source = if let Some(rel) = version.strip_prefix("file:") {
            PackageSource::Link { target_rel: PathBuf::from(rel) }
        } else if version.contains("workspace:") || version.starts_with("link:") {
            bail!(
                "{key}: locator {locator:?} uses an unsupported protocol for the Rust-native installer (file: and registry versions only)"
            );
        } else {
            // The module header has always claimed a registry entry without a
            // sha512 is refused rather than downloaded on trust; until
            // R771-F1 that was true of the npm parser only, and a bun entry
            // missing its integrity was fetched unverified. It is now also
            // unaddressable — the integrity is the store key.
            let Some(integrity) = integrity else {
                bail!(
                    "{}: packages[{key}] ({locator}) has no sha512 integrity — refusing to install unverified content (W319 §4)",
                    lock_path.display()
                );
            };
            PackageSource::Registry {
                name: name.to_string(),
                version: version.to_string(),
                integrity,
                platform: platform.clone(),
                optional,
            }
        };
        // The platform gate — the one place this walk drops a lock entry on
        // purpose, and since R773-F9 also the one place it refuses an install
        // outright. A portable lock lists every platform's build, so *some*
        // entry here always fails to match; which failure is tolerable is
        // exactly what `optional` records.
        if let Some(mismatch) = platform.mismatch(host) {
            if optional != Some(false) {
                continue;
            }
            bail!(
                "EBADPLATFORM: {key} ({locator}) cannot run on this host ({}) — its `{}` is [{}] \
                 and this host's is {}; the lock marks it a required dependency, so it cannot be \
                 skipped the way an optionalDependency would be",
                host.describe(),
                mismatch.field,
                mismatch.declared.join(", "),
                mismatch.host,
            );
        }
        // bun.lock keys are relative to the root node_modules; npm's are
        // relative to the project root, which is what the walk wants.
        out.push(LockedPackage {
            key: key.clone(),
            dest_rel: dest_for(Path::new("node_modules"), key),
            source,
        });
    }
    Ok(out)
}

/// Every lockfile format this installer reads, and where each one lives.
#[derive(Debug)]
enum Lockfile {
    Bun(PathBuf),
    /// `package-lock.json` with `lockfileVersion` 2 or 3.
    Npm(PathBuf),
    /// `pnpm-lock.yaml` v9. Unlike the other two this one does not say where
    /// anything goes; the layout is derived (R771-F4, W319 §4.1).
    Pnpm(PathBuf),
}

impl Lockfile {
    fn path(&self) -> &Path {
        match self {
            Lockfile::Bun(p) | Lockfile::Npm(p) | Lockfile::Pnpm(p) => p,
        }
    }

    fn parse(&self) -> Result<Vec<LockedPackage>> {
        match self {
            Lockfile::Bun(p) => parse_bun_lock(p),
            Lockfile::Npm(p) => parse_package_lock(p),
            Lockfile::Pnpm(p) => crate::pnpm::parse_pnpm_lock(p),
        }
    }
}

/// Pick the lockfile to install from, in the order bun → npm → pnpm.
///
/// `bun.lock` wins when a project carries several: bun is what mints the
/// locks in this camp, and an incidental `package-lock.json` (a stray `npm
/// install`, a vendored example) must not silently redefine the closure of a
/// project that has been building from bun.lock all along. `pnpm-lock.yaml`
/// is last for the same reason plus one more — it is the only format whose
/// layout is *derived* rather than read, so it is the one whose result is
/// least predictable from the file alone. Reading a pnpm lock a user already
/// committed is not adopting pnpm (R757's staging note forbids making its
/// layout, catalogs and CLI the default); it is the opposite.
fn detect_lockfile(project_root: &Path) -> Result<Lockfile> {
    let bun = project_root.join("bun.lock");
    if bun.exists() {
        return Ok(Lockfile::Bun(bun));
    }
    let npm = project_root.join("package-lock.json");
    if npm.exists() {
        return Ok(Lockfile::Npm(npm));
    }
    let pnpm = project_root.join("pnpm-lock.yaml");
    if pnpm.exists() {
        return Ok(Lockfile::Pnpm(pnpm));
    }
    bail!(
        "{} has no bun.lock, package-lock.json or pnpm-lock.yaml — the Rust-native install step is lockfile-driven (W174 amendment); mint one with `mesofact-build lock {}` (or `bun install` / `npm install` / `pnpm install`), or build against an existing node_modules with --no-install",
        project_root.display(),
        project_root.display()
    )
}

/// A `package-lock.json` v3 (or v2) `packages` key is *already* the install
/// path relative to the project root — `node_modules/x`,
/// `node_modules/@babel/core/node_modules/convert-source-map`,
/// `docs/node_modules/entities`. So there is nothing to compute here, only
/// something to refuse: a lockfile is attacker-editable content in a pull
/// request, and a key of `../../.ssh` would otherwise be a write outside the
/// project root. Link *targets* are deliberately not run through this (npm's
/// `file:../sibling` legitimately points outside the root, and the bun path
/// has always allowed it) — only destinations.
fn install_path(key: &str) -> Result<PathBuf> {
    let p = Path::new(key);
    let escapes = p.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    });
    if escapes {
        bail!("install path {key:?} escapes the project root");
    }
    Ok(p.to_path_buf())
}

/// The package name a v3 key installs, i.e. everything after the last
/// `node_modules/` segment: `docs/node_modules/entities` → `entities`,
/// `node_modules/@babel/core/node_modules/convert-source-map` →
/// `convert-source-map`. Only a fallback — an entry's own `name` field wins
/// when present, which is how `npm:` aliases stay correct.
fn package_name_from_key(key: &str) -> &str {
    const NM: &str = "node_modules/";
    match key.rfind(NM) {
        Some(i) => &key[i + NM.len()..],
        None => key,
    }
}

/// Parse `package-lock.json` (`lockfileVersion` 2 or 3) into the same
/// path-keyed shape the bun.lock walk consumes (R771-F3).
///
/// **The missing-field policy (W319 §4) lives here.** `resolved` and
/// `integrity` are optional in the format, so every entry lacking them is
/// sorted into a class that is either skipped for a stated reason or refused
/// by name — never fetched on trust:
///
/// - `""` — the root project itself. Skipped: it is the tree, not a
///   dependency of it.
/// - a key with no `node_modules` segment (`docs`, `packages/ui`) — a
///   workspace member's own source directory. npm records its metadata here;
///   the thing that gets materialized is the sibling `node_modules/<name>`
///   link entry. Skipped.
/// - `"link": true` — the symlink npm creates from `node_modules` into a
///   workspace/`file:` directory. `resolved` is a *path*, not a URL, and
///   there is no integrity because nothing is fetched. Symlinked.
/// - `"inBundle": true` — a bundled dependency, already inside its parent's
///   tarball and extracted with it. Skipped; fetching it separately is what
///   npm itself declines to do.
/// - anything else missing `version`, `resolved` or `integrity` — refused,
///   with the key and the fields it does carry in the message.
///
/// Not filtered: `dev`, `optional` and `peer`. The bun.lock walk installs
/// every entry the lock lists and this stays consistent with it — pruning by
/// intent is a resolution-shaped decision (which closure do you want?), not a
/// materialization one. Platform (`os`/`cpu`) IS filtered on both paths; see
/// [`host_supports`] for why that one is different in kind.
fn parse_package_lock(lock_path: &Path) -> Result<Vec<LockedPackage>> {
    let raw = std::fs::read_to_string(lock_path)
        .with_context(|| format!("reading {}", lock_path.display()))?;
    let parsed: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", lock_path.display()))?;

    // v2 carries the same path-keyed `packages` map as v3 (plus a legacy
    // `dependencies` mirror this ignores), so it is read on the same path. v1
    // has no `packages` map at all and is a different format.
    match parsed.get("lockfileVersion").and_then(Value::as_u64) {
        Some(2 | 3) => {}
        Some(v) => bail!(
            "{}: lockfileVersion {v} is not supported — the path-keyed `packages` map this installer reads arrived in v2; re-run `npm install --lockfile-version 3`",
            lock_path.display()
        ),
        None => bail!("{}: no \"lockfileVersion\"", lock_path.display()),
    }

    let Some(packages) = parsed.get("packages").and_then(Value::as_object) else {
        bail!("{}: no \"packages\" map", lock_path.display());
    };

    let mut out = Vec::new();
    for (key, entry) in packages {
        let Some(obj) = entry.as_object() else {
            bail!("{}: packages[{key:?}] is not an object", lock_path.display());
        };
        let flag = |k: &str| obj.get(k).and_then(Value::as_bool).unwrap_or(false);

        // Skipped classes, in the order of the doc comment above.
        if key.is_empty() || !key.split('/').any(|seg| seg == "node_modules") {
            continue;
        }
        if flag("inBundle") {
            continue;
        }
        // Platform gate — npm records `os` / `cpu` on the entry itself.
        if !host_supports(entry, &Host::current()) {
            continue;
        }

        let dest_rel = install_path(key)
            .with_context(|| format!("{}: packages[{key:?}]", lock_path.display()))?;

        if flag("link") {
            let Some(target) = obj.get("resolved").and_then(Value::as_str) else {
                bail!(
                    "{}: packages[{key:?}] is \"link\": true with no \"resolved\" target directory",
                    lock_path.display()
                );
            };
            out.push(LockedPackage {
                key: key.clone(),
                dest_rel,
                source: PackageSource::Link {
                    target_rel: PathBuf::from(target.strip_prefix("file:").unwrap_or(target)),
                },
            });
            continue;
        }

        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| package_name_from_key(key));
        let Some(version) = obj.get("version").and_then(Value::as_str) else {
            bail!(
                "{}: packages[{key:?}] has no \"version\" and is not a root, workspace, link or bundled entry (fields: {})",
                lock_path.display(),
                field_names(obj)
            );
        };
        let Some(resolved) = obj.get("resolved").and_then(Value::as_str) else {
            bail!(
                "{}: packages[{key:?}] ({name}@{version}) has no \"resolved\" tarball URL. npm omits it for entries it does not fetch — the root project, workspace source directories, \"link\": true links and \"inBundle\": true bundled deps — and this entry declares none of those (fields: {}). Re-run `npm install` to refresh the lock.",
                lock_path.display(),
                field_names(obj)
            );
        };
        // Only the public registry is read. A `resolved` pointing anywhere
        // else — a private mirror, git+ssh, a local tarball — is refused by
        // name rather than quietly re-derived against registry.npmjs.org,
        // which would install a *different* package under the same identity.
        if !resolved.starts_with(&format!("{REGISTRY}/")) {
            bail!(
                "{}: packages[{key:?}] ({name}@{version}) resolves to {resolved}, which is not on {REGISTRY} — alternate registries, git and local-tarball sources are not supported by the Rust-native installer",
                lock_path.display()
            );
        }
        let Some(integrity) = obj.get("integrity").and_then(Value::as_str) else {
            bail!(
                "{}: packages[{key:?}] ({name}@{version}) has a \"resolved\" URL but no \"integrity\" — refusing to install unverified content (W319 §4)",
                lock_path.display()
            );
        };
        if !integrity.starts_with("sha512-") {
            bail!(
                "{}: packages[{key:?}] ({name}@{version}) has integrity {integrity:?}; only sha512 is verified (npm rewrites sha1 entries on `npm install`)",
                lock_path.display()
            );
        }

        out.push(LockedPackage {
            key: key.clone(),
            dest_rel,
            source: PackageSource::Registry {
                name: name.to_string(),
                version: version.to_string(),
                integrity: integrity.to_string(),
                // A `package-lock.json` is somebody else's lock, pruned by
                // platform a few lines above: every entry that reaches here
                // already runs on this host, so there is no gate left to carry
                // and nothing downstream would ask about optionality. Only the
                // bun reader — the format this installer also *writes* — fills
                // these in.
                platform: PlatformGates::default(),
                optional: None,
            },
        });
    }
    Ok(out)
}

/// The keys an entry does carry — what a refusal message needs to be
/// actionable about an entry defined by what it is missing.
fn field_names(obj: &serde_json::Map<String, Value>) -> String {
    obj.keys().cloned().collect::<Vec<_>>().join(", ")
}

/// Install `project_root`'s locked dependency closure into
/// `project_root/node_modules`. Idempotent: a marker file records the lock
/// content hash; a fresh marker short-circuits.
pub fn install(project_root: &Path) -> Result<InstallReport> {
    #[cfg(not(unix))]
    bail!(
        "the Rust-native install step is unix-only: Windows is out of scope for stage 1 (W319 §3). Install with npm or bun and build with --no-install."
    );
    // Read before the freshness short-circuit below, so a typo in the
    // override is refused on every run rather than only on the runs that
    // happen to do work.
    let chain = materialize::chain_from_env()?;
    let lock = detect_lockfile(project_root)?;
    let lock_path = lock.path();
    let lock_raw = std::fs::read(lock_path)?;
    let lock_hash = format!("{:x}", Sha512::digest(&lock_raw));
    let node_modules = project_root.join("node_modules");
    let marker = node_modules.join(".mesofact-install.json");
    if let Ok(prev) = std::fs::read_to_string(&marker) {
        if prev.trim() == lock_hash {
            return Ok(InstallReport { installed: 0, linked: 0, skipped_fresh: true });
        }
    }

    let packages = lock.parse()?;
    let store = Store::open()?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("mesofact-build")
        .build()?;

    let mut installed = 0;
    let mut linked = 0;
    for pkg in &packages {
        // `dest_rel` is project-root-relative for both formats — a package
        // under a workspace (`docs/node_modules/entities`) lands in that
        // workspace's node_modules, not the root's.
        let dest = project_root.join(&pkg.dest_rel);
        match &pkg.source {
            PackageSource::Link { target_rel } => {
                let target = project_root.join(target_rel);
                if !target.exists() {
                    bail!(
                        "{}: link target {} does not exist",
                        pkg.key,
                        target.display()
                    );
                }
                link_package(&dest, &target)?;
                linked += 1;
            }
            // `platform` and `optional` were consumed by the parser: an entry
            // that reaches the walk is one this host can run, or the parse
            // failed naming it. Nothing here re-decides that.
            PackageSource::Registry { name, version, integrity, .. } => {
                install_registry_package(
                    &client, &store, chain, &dest, name, version, integrity,
                )?;
                installed += 1;
            }
        }
    }

    std::fs::create_dir_all(&node_modules)?;
    std::fs::write(&marker, format!("{lock_hash}\n"))?;
    Ok(InstallReport { installed, linked, skipped_fresh: false })
}

/// Split a bun.lock `packages` key into the chain of package names it encodes.
///
/// Keys are install *paths* relative to `node_modules`, not package names:
/// `"typescript"` is top-level, `"@mesofact/runtime"` is one scoped package,
/// and `"@mesofact/runtime/typescript"` is a `typescript` nested under
/// `@mesofact/runtime` (a version conflict bun couldn't hoist). A segment
/// starting with `@` swallows the following segment as its scope.
fn split_lock_key(key: &str) -> Vec<String> {
    let segs: Vec<&str> = key.split('/').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if segs[i].starts_with('@') && i + 1 < segs.len() {
            out.push(format!("{}/{}", segs[i], segs[i + 1]));
            i += 2;
        } else {
            out.push(segs[i].to_string());
            i += 1;
        }
    }
    out
}

/// Materialize a lock key as a filesystem path, interposing `node_modules/`
/// between nesting levels the way npm/bun do on disk.
fn dest_for(node_modules: &Path, key: &str) -> PathBuf {
    let mut p = node_modules.to_path_buf();
    for (i, seg) in split_lock_key(key).into_iter().enumerate() {
        if i > 0 {
            p.push("node_modules");
        }
        p.push(seg);
    }
    p
}

/// The path to `target` as seen from directory `base`. Both must already be
/// canonical — the caller canonicalizes, so no `..` or symlink survives into
/// the comparison and a plain component walk is exact.
fn relative_from(base: &Path, target: &Path) -> PathBuf {
    let mut b = base.components().peekable();
    let mut t = target.components().peekable();
    while b.peek().is_some() && b.peek() == t.peek() {
        b.next();
        t.next();
    }
    let mut out = PathBuf::new();
    for _ in b {
        out.push("..");
    }
    for c in t {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Symlink `dest` at `target`, **relatively** — which is what both npm and
/// bun write, and the difference is not cosmetic: an absolute link (what this
/// wrote before R771-F3) dangles the moment the project tree is moved,
/// copied, or restored from a CI cache at a different path. Verified against
/// a real `npm install` tree; the link was the only file the two trees
/// disagreed on.
fn link_package(dest: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if dest.symlink_metadata().is_ok() {
        if dest.is_dir() && !dest.symlink_metadata()?.file_type().is_symlink() {
            std::fs::remove_dir_all(dest)?;
        } else {
            std::fs::remove_file(dest).or_else(|_| std::fs::remove_dir_all(dest))?;
        }
    }
    #[cfg(unix)]
    {
        // The link's own directory is the frame of reference, and it exists
        // by now (create_dir_all above), so both sides can canonicalize.
        let from = dest
            .parent()
            .ok_or_else(|| anyhow!("link destination {} has no parent", dest.display()))?
            .canonicalize()?;
        let to = target
            .canonicalize()
            .with_context(|| format!("link target {}", target.display()))?;
        std::os::unix::fs::symlink(relative_from(&from, &to), dest)?;
    }
    #[cfg(not(unix))]
    bail!("file: dependency links are unix-only for now");
    Ok(())
}

/// Put `name`@`version` at `dest`, via the store: the entry for `integrity`
/// is fetched and unpacked once per machine, and this only materializes a
/// copy of it into the project.
///
/// The download is verified inside [`Store::insert_tarball`] before anything
/// durable is written, so a mismatch leaves no artifact to evict — which is
/// what the old cache's poisoned-`.tgz` deletion existed to clean up.
fn install_registry_package(
    client: &reqwest::blocking::Client,
    store: &Store,
    chain: &[Strategy],
    dest: &Path,
    name: &str,
    version: &str,
    integrity: &str,
) -> Result<()> {
    let entry = store
        .ensure(integrity, || {
            // Tarball basename drops the scope: @scope/pkg → pkg-<version>.tgz.
            let basename = name.rsplit('/').next().unwrap_or(name);
            let url = format!("{REGISTRY}/{name}/-/{basename}-{version}.tgz");
            let resp = client.get(&url).send().with_context(|| format!("GET {url}"))?;
            if !resp.status().is_success() {
                bail!("GET {url} → {}", resp.status());
            }
            Ok(resp.bytes()?.to_vec())
        })
        .with_context(|| format!("{name}@{version}"))?;

    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .or_else(|_| std::fs::remove_file(dest))
            .with_context(|| format!("clearing {}", dest.display()))?;
    }
    materialize_tree(&entry, dest, chain)
        .with_context(|| format!("materializing {name}@{version} into {}", dest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_trailing_commas_outside_strings() {
        let src = r#"{ "a": [1, 2, ], "b": { "c": "x,}", }, }"#;
        let cleaned = strip_trailing_commas(src);
        let v: Value = serde_json::from_str(&cleaned).unwrap();
        assert_eq!(v["b"]["c"], "x,}");
        assert_eq!(v["a"], serde_json::json!([1, 2]));
    }

    // The base64 codec moved to `crate::store` with the integrity handling
    // that is its only caller (R771-F1); its vectors live there now.

    /// The platform gate, stated against the host rather than a fixture: the
    /// point is that exactly one of a native fan-out survives, whichever box
    /// this runs on.
    #[test]
    fn the_platform_gate_admits_this_host_and_nothing_else() {
        let (host_os, host_cpu) = host_platform_arch().expect("a host this table models");
        assert!(host_supports(&serde_json::json!({ "os": host_os, "cpu": host_cpu }), &Host::current()));

        // A real `typescript@7.0.2` fan-out, verbatim from a bun.lock: twenty
        // natives, one runnable. `"none"` is what bun writes for a platform it
        // does not model, and it must match nothing.
        let fan_out = [
            ("darwin", "arm64"),
            ("darwin", "x64"),
            ("linux", "x64"),
            ("linux", "arm64"),
            ("linux", "none"),
            ("win32", "x64"),
            ("none", "arm64"),
            ("aix", "ppc64"),
        ];
        let admitted = fan_out
            .iter()
            .filter(|(os, cpu)| host_supports(&serde_json::json!({ "os": os, "cpu": cpu }), &Host::current()))
            .count();
        assert!(admitted <= 1, "{admitted} of a native fan-out claim to run here");
    }

    /// Absent metadata admits everything — the overwhelmingly common case, and
    /// the one where a regression would empty an install rather than fatten it.
    #[test]
    fn the_platform_gate_is_silent_on_ordinary_packages() {
        assert!(host_supports(&serde_json::json!({}), &Host::current()));
        assert!(host_supports(&serde_json::json!({ "bin": { "tsc": "bin/tsc" } }), &Host::current()));
        // Shapes the walk does not understand are admitted, not dropped.
        assert!(host_supports(&serde_json::json!({ "os": 7 }), &Host::current()));
    }

    /// npm's array form, including `!` negation, which bun never writes but a
    /// `package-lock.json` does.
    #[test]
    fn the_platform_gate_reads_npms_array_form() {
        assert!(platform_field_admits(Some(&serde_json::json!(["linux", "darwin"])), "darwin"));
        assert!(!platform_field_admits(Some(&serde_json::json!(["linux"])), "darwin"));
        assert!(platform_field_admits(Some(&serde_json::json!(["!win32"])), "darwin"));
        assert!(!platform_field_admits(Some(&serde_json::json!(["!win32"])), "win32"));
        assert!(platform_field_admits(None, "darwin"));
        // A negation wins over a positive naming the same host: npm treats the
        // exclusion list as authoritative.
        assert!(!platform_field_admits(Some(&serde_json::json!(["darwin", "!darwin"])), "darwin"));
    }

    /// The gate and `check`'s native-package resolver must name the host
    /// identically — they read the same vocabulary, and a divergence would
    /// mean the installer drops the very compiler the checker then looks for.
    #[test]
    fn the_gate_names_the_host_the_way_check_does() {
        let (os, cpu) = host_platform_arch().expect("a host this table models");
        assert_eq!(
            format!("typescript-{os}-{cpu}"),
            crate::check::node_platform_arch(std::env::consts::OS, std::env::consts::ARCH)
                .map(|(p, a)| format!("typescript-{p}-{a}"))
                .unwrap()
        );
    }

    #[test]
    fn lock_keys_split_into_package_chains() {
        assert_eq!(split_lock_key("typescript"), vec!["typescript"]);
        assert_eq!(split_lock_key("@mesofact/runtime"), vec!["@mesofact/runtime"]);
        assert_eq!(
            split_lock_key("@mesofact/runtime/typescript"),
            vec!["@mesofact/runtime", "typescript"]
        );
        assert_eq!(
            split_lock_key("@mesofact/runtime/@types/node"),
            vec!["@mesofact/runtime", "@types/node"]
        );
    }

    #[test]
    fn nested_lock_keys_get_interposed_node_modules() {
        let nm = Path::new("/p/node_modules");
        assert_eq!(dest_for(nm, "typescript"), Path::new("/p/node_modules/typescript"));
        assert_eq!(
            dest_for(nm, "@mesofact/runtime"),
            Path::new("/p/node_modules/@mesofact/runtime")
        );
        assert_eq!(
            dest_for(nm, "@mesofact/runtime/typescript"),
            Path::new("/p/node_modules/@mesofact/runtime/node_modules/typescript")
        );
    }

    // ---- R771-F3: package-lock.json v3 ------------------------------------

    /// Write `body` as `name` in a fresh tempdir; the dir is returned because
    /// dropping it deletes the file.
    fn lock_file(name: &str, body: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(name);
        std::fs::write(&path, body).unwrap();
        (tmp, path)
    }

    fn by_key<'a>(pkgs: &'a [LockedPackage], key: &str) -> &'a LockedPackage {
        pkgs.iter().find(|p| p.key == key).unwrap_or_else(|| {
            let keys: Vec<&str> = pkgs.iter().map(|p| p.key.as_str()).collect();
            panic!("no entry keyed {key:?}; have {keys:?}")
        })
    }

    fn registry_of(pkg: &LockedPackage) -> (&str, &str) {
        match &pkg.source {
            PackageSource::Registry { name, version, .. } => (name, version),
            PackageSource::Link { .. } => panic!("{} is a link, not a registry entry", pkg.key),
        }
    }

    fn integrity_of(pkg: &LockedPackage) -> Option<&str> {
        match &pkg.source {
            PackageSource::Registry { integrity, .. } => Some(integrity),
            PackageSource::Link { .. } => None,
        }
    }

    /// Shaped after a real npm v3 lock: a workspace (`docs`) with its own
    /// nested copy of a dep, a scoped package, a conflict copy nested under
    /// its parent, a `link: true` workspace symlink and a bundled entry.
    const NPM_V3_LOCK: &str = r#"{
      "name": "demo",
      "version": "1.0.0",
      "lockfileVersion": 3,
      "requires": true,
      "packages": {
        "": {
          "name": "demo",
          "version": "1.0.0",
          "workspaces": ["docs"],
          "dependencies": { "@babel/core": "^7.24.0", "entities": "^4.5.0" }
        },
        "docs": {
          "name": "@demo/docs",
          "version": "0.1.0",
          "dependencies": { "entities": "^5.0.0" }
        },
        "node_modules/@babel/core": {
          "version": "7.24.0",
          "resolved": "https://registry.npmjs.org/@babel/core/-/core-7.24.0.tgz",
          "integrity": "sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==",
          "engines": { "node": ">=6.9.0" }
        },
        "node_modules/@babel/core/node_modules/convert-source-map": {
          "version": "2.0.0",
          "resolved": "https://registry.npmjs.org/convert-source-map/-/convert-source-map-2.0.0.tgz",
          "integrity": "sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb=="
        },
        "node_modules/convert-source-map": {
          "version": "1.9.0",
          "resolved": "https://registry.npmjs.org/convert-source-map/-/convert-source-map-1.9.0.tgz",
          "integrity": "sha512-cccccccccccccccccccccccccccccccccccccccc==",
          "dev": true
        },
        "node_modules/docs": {
          "resolved": "docs",
          "link": true
        },
        "node_modules/wrappy": {
          "version": "1.0.2",
          "inBundle": true,
          "license": "ISC"
        },
        "docs/node_modules/entities": {
          "version": "5.0.0",
          "resolved": "https://registry.npmjs.org/entities/-/entities-5.0.0.tgz",
          "integrity": "sha512-dddddddddddddddddddddddddddddddddddddddd==",
          "engines": { "node": ">=0.12" }
        }
      }
    }"#;

    #[test]
    fn package_lock_v3_keys_are_install_paths_verbatim() {
        let (_tmp, path) = lock_file("package-lock.json", NPM_V3_LOCK);
        let pkgs = parse_package_lock(&path).unwrap();

        // "" (root), "docs" (workspace source dir) and wrappy (bundled) are
        // skipped; the other five are materialized.
        assert_eq!(pkgs.len(), 5, "unexpected entries: {:?}", pkgs.iter().map(|p| &p.key).collect::<Vec<_>>());

        let scoped = by_key(&pkgs, "node_modules/@babel/core");
        assert_eq!(scoped.dest_rel, Path::new("node_modules/@babel/core"));
        assert_eq!(registry_of(scoped), ("@babel/core", "7.24.0"));

        // The conflict copy stays nested under its parent — the key already
        // spells the on-disk path, node_modules segments and all.
        let nested = by_key(&pkgs, "node_modules/@babel/core/node_modules/convert-source-map");
        assert_eq!(
            nested.dest_rel,
            Path::new("node_modules/@babel/core/node_modules/convert-source-map")
        );
        assert_eq!(registry_of(nested), ("convert-source-map", "2.0.0"));
        // ... alongside, not instead of, the hoisted copy at a different version.
        assert_eq!(registry_of(by_key(&pkgs, "node_modules/convert-source-map")).1, "1.9.0");

        // A non-root project path installs into that project's node_modules.
        let ws_dep = by_key(&pkgs, "docs/node_modules/entities");
        assert_eq!(ws_dep.dest_rel, Path::new("docs/node_modules/entities"));
        assert_eq!(registry_of(ws_dep), ("entities", "5.0.0"));

        let link = by_key(&pkgs, "node_modules/docs");
        assert!(integrity_of(link).is_none());
        match &link.source {
            PackageSource::Link { target_rel } => assert_eq!(target_rel, Path::new("docs")),
            PackageSource::Registry { .. } => panic!("workspace link parsed as a registry fetch"),
        }
    }

    #[test]
    fn package_lock_integrity_carries_through() {
        let (_tmp, path) = lock_file("package-lock.json", NPM_V3_LOCK);
        let pkgs = parse_package_lock(&path).unwrap();
        assert_eq!(
            integrity_of(by_key(&pkgs, "node_modules/@babel/core")),
            Some("sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==")
        );
    }

    /// `npm:` aliases install under a directory that is not the package name,
    /// so the entry's own `name` wins over the key.
    #[test]
    fn package_lock_alias_uses_the_entry_name() {
        let (_tmp, path) = lock_file(
            "package-lock.json",
            r#"{ "lockfileVersion": 3, "packages": {
                "node_modules/string-width-cjs": {
                  "name": "string-width",
                  "version": "4.2.3",
                  "resolved": "https://registry.npmjs.org/string-width/-/string-width-4.2.3.tgz",
                  "integrity": "sha512-eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee=="
                } } }"#,
        );
        let pkgs = parse_package_lock(&path).unwrap();
        assert_eq!(registry_of(&pkgs[0]), ("string-width", "4.2.3"));
        assert_eq!(pkgs[0].dest_rel, Path::new("node_modules/string-width-cjs"));
    }

    fn refusal(entry: &str) -> String {
        let (_tmp, path) = lock_file(
            "package-lock.json",
            &format!(r#"{{ "lockfileVersion": 3, "packages": {{ {entry} }} }}"#),
        );
        let err = parse_package_lock(&path).expect_err("expected a refusal");
        format!("{err:#}")
    }

    #[test]
    fn package_lock_refuses_a_fetch_without_integrity() {
        let msg = refusal(
            r#""node_modules/left-pad": { "version": "1.3.0",
               "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz" }"#,
        );
        assert!(msg.contains("node_modules/left-pad"), "{msg}");
        assert!(msg.contains("integrity"), "{msg}");
    }

    #[test]
    fn package_lock_refuses_sha1_integrity() {
        let msg = refusal(
            r#""node_modules/left-pad": { "version": "1.3.0",
               "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz",
               "integrity": "sha1-YtRJHQGwB4z8m2vRvBpNLbTAWnQ=" }"#,
        );
        assert!(msg.contains("sha512"), "{msg}");
    }

    /// The §4 finding: entries with no `resolved` are real and common. The
    /// ones with a documented reason are skipped above; an unexplained one is
    /// named, not guessed at.
    #[test]
    fn package_lock_refuses_an_unexplained_missing_resolved() {
        let msg = refusal(r#""node_modules/mystery": { "version": "1.0.0", "dev": true }"#);
        assert!(msg.contains("node_modules/mystery"), "{msg}");
        assert!(msg.contains("resolved"), "{msg}");
        // The message lists what the entry *does* carry, so the reader can
        // classify it without opening the lock.
        assert!(msg.contains("version, dev"), "{msg}");
    }

    #[test]
    fn package_lock_refuses_a_foreign_registry() {
        let msg = refusal(
            r#""node_modules/internal": { "version": "1.0.0",
               "resolved": "https://npm.corp.example.com/internal/-/internal-1.0.0.tgz",
               "integrity": "sha512-ffffffffffffffffffffffffffffffffffffffff==" }"#,
        );
        assert!(msg.contains("npm.corp.example.com"), "{msg}");
    }

    #[test]
    fn package_lock_refuses_a_key_escaping_the_project_root() {
        let msg = refusal(
            r#""node_modules/../../evil": { "version": "1.0.0",
               "resolved": "https://registry.npmjs.org/evil/-/evil-1.0.0.tgz",
               "integrity": "sha512-gggggggggggggggggggggggggggggggggggggggg==" }"#,
        );
        assert!(msg.contains("escapes the project root"), "{msg}");
    }

    #[test]
    fn package_lock_refuses_a_v1_lock() {
        let (_tmp, path) = lock_file(
            "package-lock.json",
            r#"{ "lockfileVersion": 1, "dependencies": { "left-pad": { "version": "1.3.0" } } }"#,
        );
        let msg = format!("{:#}", parse_package_lock(&path).unwrap_err());
        assert!(msg.contains("lockfileVersion 1"), "{msg}");
    }

    #[test]
    fn package_lock_v2_reads_on_the_v3_path() {
        let (_tmp, path) = lock_file(
            "package-lock.json",
            r#"{ "lockfileVersion": 2,
                 "dependencies": { "left-pad": { "version": "1.3.0" } },
                 "packages": { "node_modules/left-pad": {
                   "version": "1.3.0",
                   "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz",
                   "integrity": "sha512-hhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhh==" } } }"#,
        );
        let pkgs = parse_package_lock(&path).unwrap();
        assert_eq!(registry_of(&pkgs[0]), ("left-pad", "1.3.0"));
    }

    #[test]
    fn package_names_come_from_the_last_node_modules_segment() {
        assert_eq!(package_name_from_key("node_modules/entities"), "entities");
        assert_eq!(package_name_from_key("node_modules/@babel/core"), "@babel/core");
        assert_eq!(
            package_name_from_key("node_modules/@babel/core/node_modules/convert-source-map"),
            "convert-source-map"
        );
        assert_eq!(package_name_from_key("docs/node_modules/entities"), "entities");
    }

    // ---- the shared walk contract ----------------------------------------

    /// The platform gate reads the PINNED host, not this machine.
    ///
    /// This is what makes R773-F7's conformance corpus machine-independent: a
    /// bun.lock is portable and lists every platform variant of a native
    /// optional, so which subset comes back is purely a function of the host
    /// asked about — and so is the lock this crate now writes (R773-F9).
    ///
    /// This is also the bun-authored shape: no entry states `optional`, so
    /// every mismatch here is a skip. That is the compatibility this installer
    /// cannot lose — a lock `bun install` wrote must keep installing.
    ///
    /// Deliberately asserts on THREE hosts rather than one, so the test states
    /// a property instead of a coincidence — and none of the assertions depends
    /// on which box runs it.
    #[test]
    fn the_bun_platform_gate_reads_the_host_it_is_given_not_this_machine() {
        let (_tmp, path) = lock_file(
            "bun.lock",
            r#"{
  "lockfileVersion": 1,
  "packages": {
    "@esbuild/linux-x64": ["@esbuild/linux-x64@0.24.0", "", { "os": "linux", "cpu": "x64" }, "sha512-linuxx64"],
    "@esbuild/darwin-arm64": ["@esbuild/darwin-arm64@0.24.0", "", { "os": "darwin", "cpu": "arm64" }, "sha512-darwinarm"],
    "@esbuild/win32-x64": ["@esbuild/win32-x64@0.24.0", "", { "os": "win32", "cpu": "x64" }, "sha512-win32x64"],
    "esbuild": ["esbuild@0.24.0", "", {}, "sha512-esbuild"]
  }
}"#,
        );

        let for_host = |os, cpu, libc| {
            let host = Host { os: Some(os), cpu: Some(cpu), libc };
            let mut names: Vec<String> = parse_bun_lock_for_host(&path, &host)
                .unwrap()
                .iter()
                .map(|p| p.key.clone())
                .collect();
            names.sort();
            names
        };

        assert_eq!(
            for_host("linux", "x64", Some("glibc")),
            ["@esbuild/linux-x64", "esbuild"],
            "a host that is not this machine"
        );
        assert_eq!(
            for_host("darwin", "arm64", None),
            ["@esbuild/darwin-arm64", "esbuild"]
        );
        assert_eq!(for_host("win32", "x64", None), ["@esbuild/win32-x64", "esbuild"]);
    }

    /// R773-F9's install-time half. The resolver no longer refuses to lock a
    /// package this machine cannot run, so this layer has to — and only for an
    /// entry the lock says is *required*. Both halves in one test, over one
    /// lock, because the pair is the rule: drop either and the remaining one
    /// reads as "skip everything" or "refuse everything".
    #[test]
    fn a_required_platform_mismatch_is_an_error_and_an_optional_one_is_a_skip() {
        let (_tmp, path) = lock_file(
            "bun.lock",
            r#"{
  "lockfileVersion": 1,
  "packages": {
    "@esbuild/darwin-arm64": ["@esbuild/darwin-arm64@0.24.0", "", { "os": "darwin", "cpu": "arm64", "optional": true }, "sha512-darwinarm"],
    "@img/sharp-linuxmusl-x64": ["@img/sharp-linuxmusl-x64@0.33.5", "", { "os": "linux", "cpu": "x64", "libc": "musl", "optional": true }, "sha512-musl"],
    "esbuild": ["esbuild@0.24.0", "", { "optional": false }, "sha512-esbuild"]
  }
}"#,
        );

        // linux/x64/glibc: both natives are for somewhere else, both optional,
        // both dropped — including the musl one, which `os`/`cpu` alone cannot
        // tell from a glibc build.
        let linux = Host { os: Some("linux"), cpu: Some("x64"), libc: Some("glibc") };
        let keys: Vec<String> =
            parse_bun_lock_for_host(&path, &linux).unwrap().iter().map(|p| p.key.clone()).collect();
        assert_eq!(keys, ["esbuild"]);

        // Same lock, and now the *required* entry is the one that cannot run.
        let win = Host { os: Some("win32"), cpu: Some("x64"), libc: None };
        let (_tmp2, required) = lock_file(
            "bun.lock",
            r#"{
  "lockfileVersion": 1,
  "packages": {
    "@esbuild/darwin-arm64": ["@esbuild/darwin-arm64@0.24.0", "", { "os": "darwin", "cpu": "arm64", "optional": false }, "sha512-darwinarm"]
  }
}"#,
        );
        let msg = parse_bun_lock_for_host(&required, &win).unwrap_err().to_string();
        assert!(msg.contains("EBADPLATFORM"), "{msg}");
        assert!(msg.contains("@esbuild/darwin-arm64"), "{msg}");
        assert!(msg.contains("win32/x64"), "{msg}");
        assert!(msg.contains("`os`"), "{msg}");
    }

    /// The three-valued `optional` in one assertion: a lock that does not state
    /// it is a lock that cannot demand the error. bun writes optionality on the
    /// *requester* rather than the entry, so collapsing "absent" to "required"
    /// would turn every native in a bun-authored lock into a failed install on
    /// twenty-three hosts out of twenty-four.
    #[test]
    fn an_entry_that_does_not_state_its_optionality_is_skipped_not_refused() {
        let (_tmp, path) = lock_file(
            "bun.lock",
            r#"{
  "lockfileVersion": 1,
  "packages": {
    "@esbuild/darwin-arm64": ["@esbuild/darwin-arm64@0.24.0", "", { "os": "darwin", "cpu": "arm64" }, "sha512-darwinarm"]
  }
}"#,
        );
        let win = Host { os: Some("win32"), cpu: Some("x64"), libc: None };
        assert!(parse_bun_lock_for_host(&path, &win).unwrap().is_empty());
    }

    #[test]
    fn bun_lock_keys_become_root_node_modules_paths() {
        let (_tmp, path) = lock_file(
            "bun.lock",
            r#"{
              "lockfileVersion": 1,
              "workspaces": { "": { "name": "demo" } },
              "packages": {
                "typescript": ["typescript@5.4.5", "", {}, "sha512-iiiiiiiiiiiiiiiiiiii=="],
                "@mesofact/runtime": ["@mesofact/runtime@file:packages/runtime", {}],
                "@mesofact/runtime/typescript": ["typescript@5.0.4", "", {}, "sha512-jjjjjjjjjjjjjjjjjjjj=="],
              }
            }"#,
        );
        let pkgs = parse_bun_lock(&path).unwrap();
        assert_eq!(pkgs.len(), 3);

        // Unlike npm's, a bun key is relative to the *root* node_modules — the
        // parser is what puts both formats on the same project-relative axis.
        let top = by_key(&pkgs, "typescript");
        assert_eq!(top.dest_rel, Path::new("node_modules/typescript"));
        assert_eq!(registry_of(top), ("typescript", "5.4.5"));

        let nested = by_key(&pkgs, "@mesofact/runtime/typescript");
        assert_eq!(
            nested.dest_rel,
            Path::new("node_modules/@mesofact/runtime/node_modules/typescript")
        );
        assert_eq!(registry_of(nested), ("typescript", "5.0.4"));

        let link = by_key(&pkgs, "@mesofact/runtime");
        assert_eq!(link.dest_rel, Path::new("node_modules/@mesofact/runtime"));
        match &link.source {
            PackageSource::Link { target_rel } => {
                assert_eq!(target_rel, Path::new("packages/runtime"))
            }
            PackageSource::Registry { .. } => panic!("file: dep parsed as a registry fetch"),
        }
    }

    /// R771-F1: the integrity is the store key as well as the verification,
    /// so a registry entry without one is refused at parse time on the bun
    /// path too (it used to be fetched and installed unverified).
    #[test]
    fn bun_lock_refuses_a_registry_entry_without_integrity() {
        let (_tmp, path) = lock_file(
            "bun.lock",
            r#"{
              "lockfileVersion": 1,
              "packages": { "typescript": ["typescript@5.4.5", "", {}] }
            }"#,
        );
        let msg = parse_bun_lock(&path).unwrap_err().to_string();
        assert!(msg.contains("typescript"), "{msg}");
        assert!(msg.contains("integrity"), "{msg}");
    }

    #[test]
    fn bun_lock_wins_when_a_project_carries_both_locks() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package-lock.json"), "{}").unwrap();
        assert!(matches!(detect_lockfile(tmp.path()).unwrap(), Lockfile::Npm(_)));
        std::fs::write(tmp.path().join("bun.lock"), "{}").unwrap();
        assert!(matches!(detect_lockfile(tmp.path()).unwrap(), Lockfile::Bun(_)));
    }

    #[test]
    fn workspace_links_are_written_relative() {
        assert_eq!(
            relative_from(Path::new("/p/node_modules/@e2e"), Path::new("/p/docs")),
            Path::new("../../docs")
        );
        assert_eq!(
            relative_from(Path::new("/p/node_modules"), Path::new("/p/node_modules/x")),
            Path::new("x")
        );
        assert_eq!(relative_from(Path::new("/p/a"), Path::new("/p/a")), Path::new("."));
        assert_eq!(relative_from(Path::new("/p/node_modules"), Path::new("/q")), Path::new("../../q"));
    }

    #[test]
    fn link_package_writes_a_relocatable_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/package.json"), "{}").unwrap();
        let dest = root.join("node_modules/@e2e/docs");
        link_package(&dest, &root.join("docs")).unwrap();
        assert_eq!(std::fs::read_link(&dest).unwrap(), Path::new("../../docs"));
        assert!(dest.join("package.json").exists(), "the relative link does not resolve");
    }

    #[test]
    fn no_lockfile_names_both_formats() {
        let tmp = tempfile::tempdir().unwrap();
        let msg = format!("{:#}", detect_lockfile(tmp.path()).unwrap_err());
        assert!(msg.contains("bun.lock") && msg.contains("package-lock.json"), "{msg}");
    }
}
