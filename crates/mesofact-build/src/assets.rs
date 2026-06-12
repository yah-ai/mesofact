//! Static-asset discovery (R490-F4) — Rust mirror of
//! `packages/mesofact-build/src/static-assets.ts`. Walk the workload's
//! public/ dir, copy files verbatim into `dist/html/`, return sorted
//! manifest entries.

use anyhow::{Context, Result};
use mesofact::manifest::StaticAsset;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const DEFAULT_PUBLIC_DIR: &str = "public";

pub fn content_type_for(rel_path: &str) -> &'static str {
    let ext = rel_path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

pub fn discover_static_assets(
    project_root: &Path,
    out_dir: &Path,
    public_dir: &str,
) -> Result<Vec<StaticAsset>> {
    let src_root = project_root.join(public_dir);
    if !src_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    walk(&src_root, "", &mut keys)?;
    keys.sort();

    let html_dir = out_dir.join("html");
    let mut assets = Vec::new();
    for key in keys {
        let bytes = std::fs::read(src_root.join(&key))
            .with_context(|| format!("reading public asset {key}"))?;
        let dest = html_dir.join(&key);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &bytes).with_context(|| format!("copying public asset {key}"))?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        assets.push(StaticAsset {
            key: key.clone(),
            content_hash: format!("{:x}", hasher.finalize()),
            content_type: content_type_for(&key).to_string(),
            immutable: false,
        });
    }
    Ok(assets)
}

fn walk(abs_dir: &Path, rel_prefix: &str, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(abs_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if rel_prefix.is_empty() { name.clone() } else { format!("{rel_prefix}/{name}") };
        let ty = entry.file_type()?;
        if ty.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else if ty.is_file() {
            out.push(rel);
        }
    }
    Ok(())
}
