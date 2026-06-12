//! Behavioral-equivalence diff (R450-F2, in-repo half). Compares two dist/
//! trees — typically `--legacy-bun` output vs the Rust-native output —
//! modulo the differences the migration plan tolerates:
//!
//! - `build_id` values (timestamps unless pinned),
//! - content-hash segments in hydrate bundle names (different bundlers ⇒
//!   different bytes ⇒ different hashes; the *naming shape* must match),
//! - JS bundle bytes themselves (compared by existence + role, not bytes).
//!
//! HTML is canonicalized (build-id + hash scrub) and then byte-compared:
//! both pipelines run the same render code, so anything beyond the scrubbed
//! tokens is a real behavioral divergence. The QED-hosted harness (R450-F2
//! proper) wires this into CI against yah-marketing + yah-dashboard.

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

pub struct DiffReport {
    pub findings: Vec<String>,
}

impl DiffReport {
    pub fn is_equivalent(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Scrub tokens equivalence tolerates: 1) the build id (read from each
/// manifest), 2) `.<hash>.js` / `.chunk-<hash>.js` segments in hydrate
/// script references.
fn canonicalize_html(html: &str, build_id: &str) -> String {
    let mut out = html.replace(build_id, "{BUILD_ID}");
    out = scrub_hashes(&out);
    out
}

fn scrub_hashes(s: &str) -> String {
    // Replace `.{token}.js` where token is 6-24 [a-z0-9_-] chars and not a
    // recognizable word like "chunk" — covers bun's and rolldown's hash
    // alphabets without regex deps.
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(".js") {
        let (head, tail) = rest.split_at(pos);
        // Walk back over one dotted token.
        let scrubbed = scrub_tail_token(head);
        out.push_str(&scrubbed);
        out.push_str(".js");
        rest = &tail[3..];
    }
    out.push_str(rest);
    out
}

fn scrub_tail_token(head: &str) -> String {
    let Some(dot) = head.rfind('.') else { return head.to_string() };
    let token = &head[dot + 1..];
    // `name.chunk-<hash>.js` first — the generic branch below would swallow
    // the whole `chunk-<hash>` token otherwise.
    if let Some(chunk_hash) = token.strip_prefix("chunk-") {
        let hashy = chunk_hash.len() >= 6
            && chunk_hash.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        return if hashy {
            format!("{}.chunk-{{HASH}}", &head[..dot])
        } else {
            head.to_string()
        };
    }
    // Bun hashes are base32-ish lowercase (may contain no digit — e.g.
    // `ervzmehh`); rolldown's are mixed-case alphanumeric. Gate on length +
    // charset and deny the word-tokens that legitimately sit in that
    // position.
    const WORD_TOKENS: &[&str] =
        &["min", "chunk", "client", "server", "bundle", "module", "spa", "app", "worker"];
    let is_hashy = token.len() >= 6
        && token.len() <= 24
        && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !WORD_TOKENS.contains(&token);
    if is_hashy {
        format!("{}.{{HASH}}", &head[..dot])
    } else {
        head.to_string()
    }
}

pub fn diff_dists(legacy: &Path, native: &Path) -> Result<DiffReport> {
    let mut findings = Vec::new();

    let legacy_manifest: Value = read_json(&legacy.join("manifest.json"))?;
    let native_manifest: Value = read_json(&native.join("manifest.json"))?;
    let legacy_id = legacy_manifest["build_id"].as_str().unwrap_or_default().to_string();
    let native_id = native_manifest["build_id"].as_str().unwrap_or_default().to_string();

    // 1. Same HTML file set.
    let legacy_html = list_files(&legacy.join("html"), "html")?;
    let native_html = list_files(&native.join("html"), "html")?;
    for missing in legacy_html.difference(&native_html) {
        findings.push(format!("html/{missing}: emitted by legacy, missing in native"));
    }
    for extra in native_html.difference(&legacy_html) {
        findings.push(format!("html/{extra}: emitted by native, missing in legacy"));
    }

    // 2. Canonicalized HTML byte-compare.
    for name in legacy_html.intersection(&native_html) {
        let l = std::fs::read_to_string(legacy.join("html").join(name))?;
        let n = std::fs::read_to_string(native.join("html").join(name))?;
        let lc = canonicalize_html(&l, &legacy_id);
        let nc = canonicalize_html(&n, &native_id);
        if lc != nc {
            findings.push(format!("html/{name}: bodies differ after canonicalization{}", first_divergence(&lc, &nc)));
        }
    }

    // 3. Manifest routes (normalized): same routes, modes, entrypoints,
    // cache policies, prefixes; hydration script names compared shape-only.
    let l_routes = normalize_manifest(&legacy_manifest);
    let n_routes = normalize_manifest(&native_manifest);
    if l_routes != n_routes {
        findings.push(format!(
            "manifest routes differ after normalization:\n  legacy: {l_routes}\n  native: {n_routes}"
        ));
    }

    // 4. Server bundle set.
    let legacy_server = list_files(&legacy.join("server"), "js")?;
    let native_server = list_files(&native.join("server"), "js")?;
    if legacy_server != native_server {
        findings.push(format!(
            "server bundle sets differ: legacy={legacy_server:?} native={native_server:?}"
        ));
    }

    Ok(DiffReport { findings })
}

fn first_divergence(a: &str, b: &str) -> String {
    let pos = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let start = pos.saturating_sub(40);
    let mut out = String::new();
    let _ = write!(
        out,
        "\n    legacy[{pos}]: …{}…\n    native[{pos}]: …{}…",
        a.get(start..(pos + 40).min(a.len())).unwrap_or("").escape_debug(),
        b.get(start..(pos + 40).min(b.len())).unwrap_or("").escape_debug(),
    );
    out
}

fn read_json(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn list_files(dir: &Path, ext: &str) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    collect(dir, "", ext, &mut out)?;
    Ok(out)
}

fn collect(dir: &Path, prefix: &str, ext: &str, out: &mut BTreeSet<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if entry.file_type()?.is_dir() {
            collect(&entry.path(), &rel, ext, out)?;
        } else if name.ends_with(&format!(".{ext}")) {
            out.insert(rel);
        }
    }
    Ok(())
}

/// Project the manifest down to the equivalence-relevant view: route →
/// (mode, render_entrypoint, cache_policy, prerender, data_inputs,
/// placement, resilience, hydration-shape) + top-level ssr_prefixes +
/// static_assets keys/hashes.
fn normalize_manifest(m: &Value) -> Value {
    let mut routes: Vec<Value> = Vec::new();
    if let Some(arr) = m["routes"].as_array() {
        for r in arr {
            let mut r2 = r.clone();
            if let Some(h) = r2.get_mut("hydration") {
                if let Some(script) = h.get("script").and_then(Value::as_str) {
                    h["script"] = Value::String(scrub_hashes(script));
                }
                if let Some(cs) = h.get("code_split").and_then(Value::as_array) {
                    let n: Vec<Value> = cs
                        .iter()
                        .map(|v| {
                            Value::String(scrub_hashes(v.as_str().unwrap_or_default()))
                        })
                        .collect();
                    h["code_split"] = Value::Array(n);
                }
            }
            routes.push(r2);
        }
    }
    routes.sort_by_key(|r| r["route"].as_str().unwrap_or_default().to_string());
    serde_json::json!({
        "routes": routes,
        "ssr_prefixes": m.get("ssr_prefixes").cloned().unwrap_or(Value::Null),
        "static_assets": m.get("static_assets").cloned().unwrap_or(Value::Null),
        "error_routes": m.get("error_routes").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_hashed_script_names() {
        let html = r#"<script type="module" src="/b1/hydrate/issues.a1b2c3d4e5f6.js"></script>"#;
        let out = canonicalize_html(html, "b1");
        assert!(out.contains("/{BUILD_ID}/hydrate/issues.{HASH}.js"), "{out}");

        let chunk = "x.chunk-a1b2c3d4.js";
        assert_eq!(scrub_hashes(chunk), "x.chunk-{HASH}.js");
    }

    #[test]
    fn leaves_plain_names_alone() {
        assert_eq!(scrub_hashes("dist/server/index.js"), "dist/server/index.js");
        assert_eq!(scrub_hashes("a.min.js"), "a.min.js");
    }
}
