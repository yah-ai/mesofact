//! Minimal npm install step (re-scoped R447). pacquet — W174's installer
//! pillar — was retired upstream (the crates.io name is a 0.0.0 placeholder;
//! orogene is similarly dormant), so the pipeline carries its own small
//! installer instead of an off-the-shelf one. Deliberately narrow scope,
//! justified by the R446 audit (15 packages, pure JS, zero install scripts):
//!
//! - **bun.lock-driven only.** Exact versions + sha512 integrity come from
//!   the existing lockfile; this step never resolves semver ranges. No lock
//!   → error, pointing at `bun install` (or a committed lockfile).
//! - **Flat layout.** Every locked package lands at `node_modules/<name>`.
//!   The audited dep surface is conflict-free; a version conflict fails the
//!   install rather than silently nesting.
//! - **No lifecycle scripts.** install/postinstall are skipped uncondition-
//!   ally (the W174 "skip-by-default" policy; the audit found zero users).
//! - **`file:` deps symlink** to their target, matching bun's behavior for
//!   workspace-style links (`@mesofact/runtime`).

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha512};
use std::io::Read;
use std::path::{Path, PathBuf};

const REGISTRY: &str = "https://registry.npmjs.org";

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
                    // skip the comma
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

struct LockedPackage {
    name: String,
    /// `name@version` or `name@file:<path>`.
    locator: String,
    integrity: Option<String>,
}

fn parse_bun_lock(lock_path: &Path) -> Result<Vec<LockedPackage>> {
    let raw = std::fs::read_to_string(lock_path)
        .with_context(|| format!("reading {}", lock_path.display()))?;
    let parsed: Value = serde_json::from_str(&strip_trailing_commas(&raw))
        .with_context(|| format!("parsing {} (after JSONC strip)", lock_path.display()))?;
    let Some(packages) = parsed.get("packages").and_then(Value::as_object) else {
        bail!("{}: no \"packages\" map", lock_path.display());
    };
    let mut out = Vec::new();
    for (name, entry) in packages {
        let Some(arr) = entry.as_array() else {
            bail!("{}: packages[{name}] is not an array", lock_path.display());
        };
        let Some(locator) = arr.first().and_then(Value::as_str) else {
            bail!("{}: packages[{name}] has no locator", lock_path.display());
        };
        // bun.lock entry shapes: [locator, registry?, meta, integrity] for
        // registry packages; [locator, meta] for file:/workspace links.
        let integrity = arr.iter().rev().find_map(Value::as_str).and_then(|s| {
            s.starts_with("sha512-").then(|| s.to_string())
        });
        out.push(LockedPackage {
            name: name.clone(),
            locator: locator.to_string(),
            integrity,
        });
    }
    Ok(out)
}

/// Install `project_root`'s locked dependency closure into
/// `project_root/node_modules`. Idempotent: a marker file records the lock
/// content hash; a fresh marker short-circuits.
pub fn install(project_root: &Path) -> Result<InstallReport> {
    let lock_path = project_root.join("bun.lock");
    if !lock_path.exists() {
        bail!(
            "{} has no bun.lock — the Rust-native install step is lockfile-driven (W174 amendment); run `bun install` once to mint the lock, or build against an existing node_modules with --no-install",
            project_root.display()
        );
    }
    let lock_raw = std::fs::read(&lock_path)?;
    let lock_hash = format!("{:x}", Sha512::digest(&lock_raw));
    let node_modules = project_root.join("node_modules");
    let marker = node_modules.join(".mesofact-install.json");
    if let Ok(prev) = std::fs::read_to_string(&marker) {
        if prev.trim() == lock_hash {
            return Ok(InstallReport { installed: 0, linked: 0, skipped_fresh: true });
        }
    }

    let packages = parse_bun_lock(&lock_path)?;
    let cache_dir = cache_root()?;
    std::fs::create_dir_all(&cache_dir)?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("mesofact-build")
        .build()?;

    let mut installed = 0;
    let mut linked = 0;
    for pkg in &packages {
        let dest = dest_for(&node_modules, &pkg.name);
        // Locator forms: "<name>@<version>" | "<name>@file:<path>" — find
        // the @ separating name from source (names may start with @scope/).
        let at = pkg.locator.rfind('@').filter(|&i| i > 0).ok_or_else(|| {
            anyhow!("unparseable locator {:?} for {}", pkg.locator, pkg.name)
        })?;
        // The *registry* name comes from the locator, never from the lock
        // key: the key is an install path, so a nested entry keyed
        // "@mesofact/runtime/typescript" would otherwise be fetched as a
        // package of that name (a guaranteed 404).
        let registry_name = &pkg.locator[..at];
        let source = &pkg.locator[at + 1..];
        if let Some(rel) = source.strip_prefix("file:") {
            let target = project_root.join(rel);
            if !target.exists() {
                bail!(
                    "{}: file: dependency target {} does not exist",
                    pkg.name,
                    target.display()
                );
            }
            link_package(&dest, &target)?;
            linked += 1;
        } else if source.contains("workspace:") || source.starts_with("link:") {
            bail!(
                "{}: locator {:?} uses an unsupported protocol for the Rust-native installer (file: and registry versions only)",
                pkg.name,
                pkg.locator
            );
        } else {
            install_registry_package(&client, &cache_dir, &dest, registry_name, source, pkg.integrity.as_deref())?;
            installed += 1;
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

fn cache_root() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| anyhow!("neither XDG_CACHE_HOME nor HOME is set"))?;
    Ok(base.join("mesofact").join("npm"))
}

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
    std::os::unix::fs::symlink(target.canonicalize()?, dest)?;
    #[cfg(not(unix))]
    bail!("file: dependency links are unix-only for now");
    Ok(())
}

fn install_registry_package(
    client: &reqwest::blocking::Client,
    cache_dir: &Path,
    dest: &Path,
    name: &str,
    version: &str,
    integrity: Option<&str>,
) -> Result<()> {
    // Tarball basename drops the scope: @scope/pkg → pkg-<version>.tgz.
    let basename = name.rsplit('/').next().unwrap_or(name);
    let cache_file = cache_dir.join(format!(
        "{}-{version}.tgz",
        name.replace('/', "+")
    ));
    let bytes = if cache_file.exists() {
        std::fs::read(&cache_file)?
    } else {
        let url = format!("{REGISTRY}/{name}/-/{basename}-{version}.tgz");
        let resp = client.get(&url).send().with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!("GET {url} → {}", resp.status());
        }
        let bytes = resp.bytes()?.to_vec();
        std::fs::write(&cache_file, &bytes)?;
        bytes
    };

    if let Some(expected) = integrity {
        let got = format!(
            "sha512-{}",
            base64_encode(&Sha512::digest(&bytes))
        );
        if got != expected {
            // Poisoned cache or registry mismatch — drop the cache entry so
            // a retry re-downloads.
            let _ = std::fs::remove_file(&cache_file);
            bail!("{name}@{version}: integrity mismatch (expected {expected}, got {got})");
        }
    }

    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .or_else(|_| std::fs::remove_file(dest))
            .with_context(|| format!("clearing {}", dest.display()))?;
    }
    std::fs::create_dir_all(dest)?;

    let gz = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        // npm tarballs root everything under a single dir (almost always
        // "package/"); strip that first component whatever it's called.
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(&stripped);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            std::fs::write(&out_path, buf)?;
        }
    }
    Ok(())
}

// Standard (non-url-safe, padded) base64 — npm integrity strings use it.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
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

    #[test]
    fn base64_matches_known_vector() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"ab"), "YWI=");
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
}
