//! Static-asset discovery (R490-F4) — Rust mirror of
//! `packages/mesofact-build/src/static-assets.ts`. Walk the workload's
//! public/ dir, copy files verbatim into `dist/html/`, return sorted
//! manifest entries.
//!
//! @yah:relay(R821, "Two build-output gaps a real WASM site hits: .wasm MIME and spa routes missing from the sitemap")
//! @yah:at(2026-08-16T22:31:44Z)
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
//!
//! @yah:relay(R825, "mesofact-static assets ship with no cache validators and no compression")
//! @yah:at(2026-10-10T00:55:09Z)
//! @yah:next("Measured 2026-10-10 00:45Z on noisetable.com/app (a mesofact-static component in the merged bundle, mesofact serve 0.8.41-r931b8): no Cache-Control, ETag or Last-Modified on any asset, and `Accept-Encoding: br, gzip` still gets identity bytes (35,338,224 B wasm). A browser cannot even revalidate, so every visit re-downloads everything. serve emits `immutable` only for SSR instance pointers (server.rs IMMUTABLE_CACHE_CONTROL); assets.rs hard-codes `immutable: false`.")
//! @yah:next("Consumer: noisetable camp R822 (board path ~/ss/noisetable). There the 35 MB wasm moves to cdn.noisetable.com (R822-F1), but index.html, the JS shim, snippets/ and the worklet stay on mesofact, so this relay is what makes them cacheable.")
//!
//! @yah:ticket(R825-F1, "Static assets: content-hash ETag + no-cache default, declared immutable globs, precompressed br/gzip")
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-glimmerstone)
//! @yah:at(2026-10-10T01:54:33Z)
//! @yah:parent(R825)
//! @yah:next("(1) Validators everywhere: serve every static asset with ETag = its content_hash and answer If-None-Match with 304; default Cache-Control `no-cache` (revalidate, never stale).")
//! @yah:next("(2) Immutability is DECLARED, not sniffed: a component-level glob list (e.g. workload.toml `immutable = [\"noise_table_browser-*.js\", \"*_bg.wasm\"]`) sets StaticAsset.immutable, which serve maps to `public, max-age=31536000, immutable`. Unlisted files keep no-cache.")
//! @yah:next("(3) Compression at build/publish time, never per request: store br + gzip variants, negotiate on Accept-Encoding, send Vary: Accept-Encoding. On-the-fly compression of a 35 MB wasm per request is not acceptable.")
//! @yah:verify("curl -sI https://noisetable.com/app/<hashed>.js → cache-control immutable + etag; /app/ index.html → no-cache + etag; repeat with If-None-Match → 304.")
//! @yah:verify("curl -sI --compressed on the JS → content-encoding br; Vary: Accept-Encoding present.")
//! @yah:gotcha("yubaba's reconciler/r2_publish.rs::cache_control_for already decides immutability for the R2 publish path from a hash-shaped filename under hydrate/. Do not add a third rule: make the declared list feed both paths, or record why it can't.")
//! @yah:gotcha("Trunk's wasm-bindgen snippets dir (snippets/<crate>-<hash>/) is believed to be keyed on crate name+version, not contents, so it must NOT match an immutable glob even though it looks hashed. Verify before trusting it.")
//! @yah:assumes("Assumes mesofact-static component files reach serve as manifest StaticAssets built here. The R870-B11 merged bundle may assemble them in yubaba instead; find the real seam before editing.")
//! @yah:handoff("SEAM FOUND (resolves the assumes): noisetable's /app files never pass through mesofact-build. They are Trunk output staged by yah cli assemble_multi_component_bundle -> yah_mesofact_bundle::collect_component_files at app/dist/html/app/, and serve answers from disk via server.rs serve_static, which never read any manifest. So the carrier is an ASSET INDEX inside the served tree (.mesofact-assets.json: sha256, size, immutable, encodings per file), written at publish time by the bundle crate and read by serve. It stages at the mount along with the component, so serve finds it by walking up from a file with no mount awareness.")
//! @yah:handoff("NEW oss/yah-base/crates/mesofact-bundle/src/assets.rs: ImmutableGlobs (whole-path match, * and ? within a segment, ** across segments; refuses abs, .., empty segments and [ ]), declared_immutable, OutputPolicy (the one shape->rule mapping), is_content_hashed_hydrate_output (moved out of yubaba r2_publish), finalize_served_dir / finalize_component_output (index plus .br/.gz siblings; brotli q11 below 4 MiB, q9 above; gzip -9; a variant is kept only if it is <=95% of the identity size; reuse by sha256 so an unchanged file never recompresses; deletes orphaned variants it created). Called from all three assembly entry points: collect_component_files, assemble_vanilla_bundle, assemble_self_bundle_with.")
//! @yah:handoff("SERVE: new oss/mesofact/crates/mesofact/src/asset_response.rs, used by both serve_static branches (html root and hydrate root). ETag = sha256 (identity) or sha256.br / sha256.gzip (variants). If-None-Match returns a 304 that carries ETag, Cache-Control and Vary. Cache-Control is public, max-age=31536000, immutable when declared, otherwise no-cache. Accept-Encoding negotiation honours q-values; a request with no Accept-Encoding gets identity. Vary: accept-encoding goes on every representation of a file that has variants. An unindexed tree (dev, a hand-run serve) still gets an on-demand sha256 ETag, cached by (len, mtime). The index file itself returns 404.")
//! @yah:handoff("DECLARATION LIVES IN mesofact.config.toml [build] immutable, NOT workload.toml (judgment call, reversible by changing one path in declared_immutable): workload_spec::BuildConfig is deny_unknown_fields AND rides the kamaji postcard wire (kamaji-proto codec, ~17 struct literals across yah/yubaba/kamaji). A key there would be a fleet wire change to carry a fact no node reads. mesofact.config.toml is loosely parsed and read only at build/assembly time.")
//! @yah:handoff("GOTCHA HONOURED, ONE RULE: yubaba r2_publish::cache_control_for now delegates to OutputPolicy (declared patterns plus the hydrate rule). publish_to_r2 takes &OutputPolicy, which mesofact_static.rs loads from the workload dir and app/yah/cli/src/qed_publish.rs passes as default. It also skips finalize artifacts, because CF negotiates encoding at its own edge.")
//! @yah:handoff("ALSO: mesofact-build StaticAsset.immutable now comes from the same declaration (was hard-coded false). mesofact-core CachePolicyTable::apply now treats 304 like 200, because a cache replaces stored headers with the 304's (RFC 9111 §4.3.4); without that, one revalidation would erase a route's declared TTL. BundleError gained AssetPolicy. No bundle-contract bump: an old runtime serves the same identity bytes, and the new runtime hashes on demand for an unindexed bundle.")
//! @yah:handoff("LIVE ROLLOUT, NOT DONE — needs operator and noisetable steps, no mesofact code: (1) ship a mesofact runtime plus yah CLI built from this tree. The CDN was still at 0.8.43 when checked; a yah-release-wizard QED run was in flight. (2) In the noisetable camp (R822), add app/browser/mesofact.config.toml containing: [build] immutable = [\"noise_table_browser-*.js\", \"*_bg.wasm\"]. (3) Re-apply noisetable-marketing. Assembly then writes the index and variants into app/browser/dist in place.")
//! @yah:verify("cargo test -p yah-mesofact-bundle (oss/yah-base): 48 pass. Covers glob semantics including snippets/ unreachable by a bare pattern, finalize, variant round-trip decode, reuse sentinel, orphan cleanup, mesofact-build shape (html + hydrate roots), and the updated collect_component_files path lists that now include the index.")
//! @yah:verify("cargo test -p mesofact --lib: 121 pass. 4 new router tests: a declared immutable asset under /app returns br, an immutable Cache-Control, the variant ETag and Vary; an undeclared file is no-cache and returns 304 on its ETag but 200 for a different representation's tag; an unindexed file still gets a sha256 ETag and a 304; the index file returns 404. cargo check -p mesofact --no-default-features: ok.")
//! @yah:verify("cargo test -p mesofact-build -p mesofact-core -p mesofact-dev: all pass (mesofact-build lib 112, plus the new manifest_immutable_flag test and the 304 cache_policy test). cargo test -p yah-cloud --lib (oss/yubaba): 1338 pass, including the new declared_patterns_reach_the_r2_path test. cargo check -p yah --lib: exit 0. cargo test -p yah --lib bundle_assembly: 18 pass, including assembly_is_deterministic.")
//! @yah:verify("E2E against a COPY of noisetable's real Trunk dist in /tmp: the real assembly path, then mesofact serve --bundle, then curl. hashed JS: 200, content-encoding br, cache-control immutable, etag <sha>.br, vary. wasm: br 9,181,377 B (was 35,245,696 identity), immutable; the decoded br has the same sha256 as the file. /app/ index.html: no-cache with an etag; If-None-Match on it -> 304, 0 B. snippets/koda-*/js/*.js: no-cache. First finalize took 11.5 s; a re-run took 99 ms (reuse). This machine's curl has no brotli, so curl --compressed correctly got gzip.")
//! @yah:verify("OPEN, operator: the ticket's two live curl lines on noisetable.com can only pass after the three LIVE ROLLOUT steps above.")
//! @yah:gotcha("Commit 3e078d5a ('release v0.8.45', 18:17) swept in a PARTIAL snapshot of this work: lib.rs declares pub mod assets, but assets.rs only landed in 2ee55b7d (18:26). So 3e078d5a alone does not build yah-mesofact-bundle. Builds read the working tree, which was consistent; the CDN showed no 0.8.45 published when checked. Build releases from 2ee55b7d or later.")
//! @yah:gotcha("Bundles grow: the wasm adds ~9.2 MB br + ~12.6 MB gz to the bundle and store. That is moot for noisetable once R822-F1 moves the wasm to the CDN, but every large compressible asset now ships three times.")
//! @yah:gotcha("The R825 snippets gotcha was not verified against wasm-bindgen's naming. It is moot by construction: patterns match whole paths, so snippets/ is reachable only by a pattern that names it. In the e2e, snippets served no-cache.")
//! @yah:cleanup("TS mirror packages/mesofact-build/src/static-assets.ts still writes immutable:false; its header now says so. Porting it needs the same glob semantics as ImmutableGlobs, not a second matcher. Production builds use the Rust binary.")
//! @yah:cleanup("mesofact-publisher's own `mesofact publish` R2 path (publish.rs cache_control_for: assets/ and hydrate/ prefixes immutable under /{build_id}/ keys) was left alone. It publishes per-build-prefixed URLs, a different key space; decide whether it should also read OutputPolicy.")

use anyhow::{Context, Result};
use mesofact_core::manifest::StaticAsset;
use std::path::Path;
use yah_mesofact_bundle::assets::{sha256_hex, ImmutableGlobs};

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

/// `immutable` is the project's declared `[build] immutable` patterns from
/// `mesofact.config.toml` (`yah_mesofact_bundle::assets::declared_immutable`,
/// the key's single reader), matched against each
/// key — a public/ key IS its path under the served `dist/html` root, which is
/// what those patterns are relative to. The manifest flag therefore states the
/// same fact the publish-time asset index serves from (MFT-R825-F1); it was a
/// hard-coded `false` before, i.e. the manifest could not express it at all.
pub fn discover_static_assets(
    project_root: &Path,
    out_dir: &Path,
    public_dir: &str,
    immutable: &ImmutableGlobs,
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
        assets.push(StaticAsset {
            content_hash: sha256_hex(&bytes),
            content_type: content_type_for(&key).to_string(),
            immutable: immutable.matches(&key),
            key,
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

    #[test]
    fn manifest_immutable_flag_is_the_declared_patterns_not_a_constant() {
        // MFT-R825-F1: was hard-coded `false`, so no manifest could say a
        // public/ asset was content-addressed even when its author knew it was.
        let project = tempfile::tempdir().unwrap();
        let out = project.path().join("dist");
        std::fs::create_dir_all(project.path().join("public/wasm")).unwrap();
        std::fs::write(project.path().join("public/wasm/app-0123abcd_bg.wasm"), b"\0asm").unwrap();
        std::fs::write(project.path().join("public/robots.txt"), b"User-agent: *").unwrap();
        let declared =
            yah_mesofact_bundle::assets::ImmutableGlobs::new(&["wasm/*_bg.wasm"]).unwrap();

        let assets = super::discover_static_assets(project.path(), &out, "public", &declared).unwrap();

        let flag = |k: &str| assets.iter().find(|a| a.key == k).unwrap().immutable;
        assert!(flag("wasm/app-0123abcd_bg.wasm"));
        assert!(!flag("robots.txt"));
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
