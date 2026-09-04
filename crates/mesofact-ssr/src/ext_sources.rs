//! Make the deno extension crates' JS reachable without the build machine (R823).
//!
//! `deno_core::extension!` lowers every declared JS file to
//! `ExtensionFileSourceCode::LoadedFromFsDuringSnapshot(<absolute path>)`, and
//! `JsRuntime::new` reads those paths off the filesystem
//! (deno_core-0.404.0/extensions.rs:156). The paths point at the COMPILING
//! machine's cargo registry, so on any other machine the read is ENOENT and
//! `JsRuntime::new` panics before the isolate exists:
//!
//! ```text
//! Failed to initialize a JsRuntime: No such file or directory (os error 2)
//!   at deno_core-0.404.0/runtime/jsruntime.rs:681
//! SSR isolate thread died during startup: receiving on a closed channel
//! ```
//!
//! [`build.rs`] copies each of those files into `OUT_DIR` at build time;
//! [`embed_sources`] swaps the paths for the embedded text before the extension
//! reaches `JsRuntime::new`. `Computed` is runtime-loadable by deno_core's own
//! definition (`ExtensionFileSource::is_runtime_loadable`), so nothing else in
//! deno_core has to know.

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use deno_core::{Extension, ExtensionFileSource, ExtensionFileSourceCode};

include!(concat!(env!("OUT_DIR"), "/ext_sources_table.rs"));

/// The table is keyed by the last two components of the path `extension!` baked
/// in — `deno_web-0.282.0/06_streams.js`. See `build.rs::source_key` for why not
/// the whole path.
fn source_key(path: &str) -> Option<String> {
    let path = Path::new(path);
    let file = path.file_name()?;
    let dir = path.parent().and_then(Path::file_name)?;
    Some(format!(
        "{}/{}",
        dir.to_string_lossy(),
        file.to_string_lossy()
    ))
}

fn embedded(key: &str) -> Option<&'static str> {
    EMBEDDED_EXT_SOURCES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, code)| *code)
}

/// Replace every filesystem-backed source in `ext` with the copy embedded at
/// build time. Sources that are already reachable at runtime (inline, or
/// `include_str!`ed by the extension crate) are left alone.
///
/// Panics if a declared file has no embedded copy: that means the extension set
/// here and the one in `build.rs` have drifted, and the alternative is the
/// silent ENOENT panic this function exists to prevent.
pub(crate) fn embed_sources(mut ext: Extension) -> Extension {
    for list in [
        &mut ext.js_files,
        &mut ext.esm_files,
        &mut ext.lazy_loaded_js_files,
        &mut ext.lazy_loaded_esm_files,
    ] {
        if list.iter().all(ExtensionFileSource::is_runtime_loadable) {
            continue;
        }
        let rewritten: Vec<ExtensionFileSource> = list
            .iter()
            .map(|file| {
                if file.is_runtime_loadable() {
                    return file.clone();
                }
                // The only non-runtime-loadable variant is the fs-backed one, so
                // this always matches; matching rather than unwrapping keeps the
                // panic message useful if deno_core adds another.
                let ExtensionFileSourceCode::LoadedFromFsDuringSnapshot(path) = &file.code else {
                    panic!(
                        "mesofact-ssr: extension source `{}` is not runtime-loadable and \
                         is not fs-backed either — deno_core grew a source variant this \
                         crate does not know how to embed",
                        file.specifier,
                    )
                };
                let code = source_key(path)
                    .as_deref()
                    .and_then(embedded)
                    .unwrap_or_else(|| {
                        panic!(
                            "mesofact-ssr: no build-time copy of extension source `{}` \
                             (declared at {path}) — build.rs did not embed it, so this \
                             binary would panic at JsRuntime::new on any machine without \
                             that exact path. If a deno_* bump moved its JS out of the \
                             crate root, build.rs's top-level glob needs to follow it.",
                            file.specifier,
                        )
                    });
                ExtensionFileSource::new_computed(file.specifier, Arc::from(code))
            })
            .collect();
        *list = Cow::Owned(rewritten);
    }
    ext
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression gate for R823, and the one that can run anywhere: after
    /// `embed_sources`, no declared file is left pointing at a path that only
    /// exists on this machine.
    #[test]
    fn every_extension_source_is_runtime_loadable() {
        let mut checked = 0usize;
        for ext in crate::ssr::extensions() {
            let ext = embed_sources(ext);
            for file in ext
                .js_files
                .iter()
                .chain(ext.esm_files.iter())
                .chain(ext.lazy_loaded_js_files.iter())
                .chain(ext.lazy_loaded_esm_files.iter())
            {
                assert!(
                    file.is_runtime_loadable(),
                    "extension source `{}` is still loaded from the build machine's \
                     filesystem — a binary shipped elsewhere will panic at JsRuntime::new",
                    file.specifier,
                );
                assert!(
                    !file.load().expect("loading embedded source").is_empty(),
                    "extension source `{}` embedded as empty",
                    file.specifier,
                );
                checked += 1;
            }
        }
        // deno_webidl(1) + deno_web(24) + deno_net(3) + deno_fetch(8) at the
        // pinned versions. A drop to zero would make the assertions above
        // vacuous, which is exactly how R823 survived: the gate ran where the
        // defect is invisible.
        assert!(
            checked >= 30,
            "only {checked} extension sources inspected — the extension set shrank \
             unexpectedly, so this test is no longer covering R823"
        );
    }

    /// The reverse direction: `build.rs` globs the extension crates' roots
    /// rather than reading their declarations, so it could in principle embed a
    /// `.js` no extension asks for. Today the two sets are equal, and this test
    /// is what says so — if a dep bump ships a stray root-level script the
    /// mismatch surfaces here instead of as unexplained binary growth.
    #[test]
    fn embedded_table_matches_declared_sources() {
        let declared: Vec<String> = crate::ssr::extensions()
            .iter()
            .flat_map(|ext| {
                ext.js_files
                    .iter()
                    .chain(ext.esm_files.iter())
                    .chain(ext.lazy_loaded_js_files.iter())
                    .chain(ext.lazy_loaded_esm_files.iter())
                    .filter_map(|f| match &f.code {
                        ExtensionFileSourceCode::LoadedFromFsDuringSnapshot(p) => source_key(p),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        for (key, _) in EMBEDDED_EXT_SOURCES {
            assert!(
                declared.iter().any(|d| d == key),
                "embedded source `{key}` is not declared by any registered extension — \
                 build.rs is embedding a file nothing loads"
            );
        }
    }
}
