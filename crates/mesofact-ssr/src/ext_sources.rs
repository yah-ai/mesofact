//! R823 regression gate: nothing the SSR isolate registers may be read off the
//! build machine's filesystem at `JsRuntime::new`.
//!
//! `deno_core::extension!` lowers every declared JS file to
//! `ExtensionFileSourceCode::LoadedFromFsDuringSnapshot(<absolute path>)`, and
//! `JsRuntime::new` reads those paths at boot (deno_core-0.404.0/extensions.rs:156)
//! — on any machine but the compiling one that is ENOENT and a panic. Until
//! R750-F1 the deno_web/deno_fetch/deno_net/deno_webidl extensions declared ~36
//! such files and a build script embedded them. Since R750-F1 the isolate
//! registers ops only and its JS is `ssr::SSR_PRELUDE` (`include_str!`), so
//! there is nothing to embed; these tests hold that line. If a future extension
//! declares `js = [...]` / `esm = [...]`, they fail rather than letting the
//! binary ship with build-machine paths.

#[cfg(test)]
mod tests {
    use deno_core::{ExtensionFileSource, ExtensionFileSourceCode};

    fn declared_sources() -> Vec<ExtensionFileSource> {
        crate::ssr::extensions()
            .into_iter()
            .flat_map(|ext| {
                ext.js_files
                    .iter()
                    .chain(ext.esm_files.iter())
                    .chain(ext.lazy_loaded_js_files.iter())
                    .chain(ext.lazy_loaded_esm_files.iter())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn every_extension_source_is_runtime_loadable() {
        for file in declared_sources() {
            assert!(
                file.is_runtime_loadable(),
                "extension source `{}` is loaded from the build machine's filesystem — \
                 a binary shipped elsewhere will panic at JsRuntime::new (R823)",
                file.specifier,
            );
        }
    }

    /// The lean set is ops-only. A JS file appearing here means someone wired
    /// an extension's JS back in; route it through `SSR_PRELUDE` instead, or
    /// bring back an embedding step and update this test deliberately.
    #[test]
    fn extensions_declare_no_fs_backed_sources() {
        let fs_backed: Vec<String> = declared_sources()
            .into_iter()
            .filter(|f| matches!(f.code, ExtensionFileSourceCode::LoadedFromFsDuringSnapshot(_)))
            .map(|f| f.specifier.to_string())
            .collect();
        assert!(fs_backed.is_empty(), "fs-backed extension sources: {fs_backed:?}");
        assert!(
            !crate::ssr::extensions().is_empty(),
            "the op extension (mesofact_fetch) must stay registered"
        );
    }
}
