//! Static-asset discovery (R490-F4) — Rust mirror of
//! `packages/mesofact-build/src/static-assets.ts`. Walk the workload's
//! public/ dir, copy files verbatim into `dist/html/`, return sorted
//! manifest entries.
//!
//! @yah:relay(R821, "Two build-output gaps a real WASM site hits: .wasm MIME and spa routes missing from the sitemap")
//! @yah:at(2026-08-16T22:31:44Z)
//! @yah:status(open)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//!
//! @yah:ticket(R821-B1, "content_type_for has no .wasm arm, so every WASM site loses instantiateStreaming")
//! @yah:status(review)
//! @yah:at(2026-08-25T06:25:13Z)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R821)
//! @yah:severity(low)
//! @yah:next("Add `\"wasm\" => \"application/wasm\"` to content_type_for (crates/mesofact-build/src/assets.rs:13) and to its TS mirror packages/mesofact-build/src/static-assets.ts, which the file's own header says to keep in lockstep.")
//! @yah:verify("A public/*.wasm asset appears in manifest.json static_assets with content_type application/wasm, and curl -sI through mesofact-dev returns it")
//! @yah:next("OBSERVED, not theorised: noisetable's web/landing (a mesofact-spa marketing site with browser-WASM audio demos) publishes a 3.9 MB public/wasm/noise_table_browser_lib_bg.wasm. It lands in the manifest as application/octet-stream and mesofact-dev serves it as that. wasm-bindgen's loader then fails WebAssembly.instantiateStreaming, console.warns 'your server does not serve Wasm with application/wasm MIME type', and falls back to arrayBuffer() + WebAssembly.instantiate — the whole 3.9 MB is buffered before compilation starts instead of compiling as it streams.")
//! @yah:gotcha("Tier: Thief — one match arm in each of two mirrored tables.")
//! @yah:handoff("Added the .wasm -> application/wasm arm to FOUR MIME tables, not the two the ticket named.")
//! @yah:verify("cargo test -p mesofact --lib — 66 passed, 0 failed (includes the new served-header test)")
//! @yah:handoff("The extra two tables are the ones that actually produce the observed symptom: crates/mesofact/src/server.rs mime_for (what mesofact-dev answers with) and crates/mesofact-publisher/src/publish.rs content_type_for (what gets PUT to R2). Both walk files off disk and NEVER read the manifest static_assets content_type, so fixing only the two build-side tables would have left dev AND production still serving octet-stream while the manifest claimed otherwise.")
//! @yah:handoff("Four regression tests, one per table: assets.rs wasm_gets_the_one_mime_instantiate_streaming_accepts; build.test.ts 'serves .wasm as application/wasm so instantiateStreaming works'; server.rs serves_wasm_with_the_mime_instantiate_streaming_accepts (asserts the SERVED Content-Type header through the router, since a correct build table proves nothing about what a browser receives); publish.rs wasm_publishes_as_application_wasm (new tests mod in that file).")
//! @yah:verify("cargo test -p mesofact-build --lib assets:: - 2 passed; cargo test -p mesofact-publisher --lib - 17 passed; bun test tests/build.test.ts in packages/mesofact-build - 31 pass, 0 fail")
//! @yah:verify("Confirmed mesofact-dev routes through the fixed code: crates/mesofact-dev/src/main.rs:72 builds mesofact::server::Server, whose serve_static (server.rs:1111) labels every disk hit with mime_for.")
//! @yah:cleanup("Pre-existing and untouched: extension matching is case-inconsistent ACROSS the four MIME tables. The two build-side tables lowercase the extension before lookup; server.rs mime_for and publisher content_type_for match the raw extension. So public/FOO.WASM gets application/wasm in the manifest but octet-stream from the dev server and from R2. Same split already applies to .PNG, .SVG and every other arm, so it is a separable behaviour change for all extensions, not a wasm fix.")
//! @yah:verify("Verified against the reporting site itself. Built /Users/leif/ss/noisetable/web/landing (its external/yah is a symlink to this tree, so it consumes the fix directly) and read dist/manifest.json: wasm/noise_table_browser_lib_bg.wasm now carries content_type application/wasm. That is the ticket verify line, met on the real 3.9 MB module.")

use anyhow::{Context, Result};
use mesofact_core::manifest::StaticAsset;
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
        // text/plain, not text/x-shellscript, and deliberately so: a shell
        // script in public/ is nearly always a `curl … | sh` installer, and the
        // whole trust posture of that pattern is "read the script before you
        // pipe it". Serving it as x-shellscript (or falling through to
        // octet-stream) makes a browser DOWNLOAD it instead of showing it.
        // Keep in lockstep with the TS mirror (R560-F1).
        "txt" | "sh" => "text/plain; charset=utf-8",
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
        // Load-bearing for every WASM site: WebAssembly.instantiateStreaming
        // REJECTS any response whose Content-Type isn't exactly
        // application/wasm. wasm-bindgen's loader catches that, warns "your
        // server does not serve Wasm with application/wasm MIME type", and
        // falls back to arrayBuffer() + instantiate() — which buffers the
        // whole module before compilation starts instead of compiling as it
        // streams. On a multi-megabyte .wasm that's the whole download
        // serialized ahead of the compile (R821-B1).
        "wasm" => "application/wasm",
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

#[cfg(test)]
mod tests {
    use super::content_type_for;

    #[test]
    fn shell_scripts_are_readable_text_not_a_download() {
        // yah.dev/install.sh is served out of public/. `curl … | sh` only earns
        // trust if a human can open the URL and read the script first, which
        // they cannot if it arrives as octet-stream and the browser saves it to
        // disk (R560-F1).
        assert_eq!(content_type_for("install.sh"), "text/plain; charset=utf-8");
        assert_eq!(content_type_for("nested/install.SH"), "text/plain; charset=utf-8");
        // Genuinely opaque extensions still fall back.
        assert_eq!(content_type_for("blob.bin"), "application/octet-stream");
    }

    #[test]
    fn wasm_gets_the_one_mime_instantiate_streaming_accepts() {
        // Anything but exactly application/wasm makes
        // WebAssembly.instantiateStreaming reject, and wasm-bindgen's loader
        // silently degrades to buffer-the-whole-module-then-compile (R821-B1).
        assert_eq!(
            content_type_for("wasm/noise_table_browser_lib_bg.wasm"),
            "application/wasm"
        );
        assert_eq!(content_type_for("UPPER.WASM"), "application/wasm");
    }
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
