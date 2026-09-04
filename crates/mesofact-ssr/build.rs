//! Embed the deno extension crates' JS sources into the binary (R823).
//!
//! `deno_core::extension!` compiles every declared `js` / `esm` /
//! `lazy_loaded_*` file down to
//! `ExtensionFileSourceCode::LoadedFromFsDuringSnapshot(<absolute path>)`
//! (deno_core-0.404.0/extensions.rs:100 — `__extension_include_js_files_detect`
//! is unconditionally `mode=loaded`). `ExtensionFileSource::load()` then does a
//! plain `std::fs::read_to_string(path)` at `JsRuntime::new` time. The path is
//! the COMPILING machine's cargo registry, so a shipped binary panicked with
//! `Failed to initialize a JsRuntime: No such file or directory (os error 2)` on
//! every machine that was not the build machine.
//!
//! This build script copies each of those files into `OUT_DIR`;
//! [`src/ext_sources.rs`] `include_str!`s them back and rewrites the extensions'
//! file lists to `ExtensionFileSourceCode::Computed` before handing them to
//! `JsRuntime::new`, which makes the fs read disappear.
//!
//! # THIS SCRIPT MUST NOT LINK deno_core, AND THAT IS THE WHOLE SHAPE OF IT
//!
//! The first cut took deno_core + the four extension crates as
//! build-dependencies and read the file lists off real `Extension` values. That
//! is more precise, and it broke the musl fleet build (R556-F6 run 8d5023a1,
//! us-west-002):
//!
//! ```text
//! error: could not compile `mesofact-ssr` (build script)
//! ld: escape-analysis.cc:(.text+0x699): undefined reference to
//!     `std::__throw_length_error(char const*)'   (…~30 more libstdc++ symbols)
//! ```
//!
//! deno_core pulls `v8`, whose `librusty_v8.a` is C++ and needs `-lstdc++`
//! `-latomic` `-lgcc` in a `--start-group`. The musl builder image supplies
//! those through `RUSTFLAGS`, and **cargo does not apply `RUSTFLAGS` to host
//! units when `--target` is passed** — a build script is a host unit. So the
//! flags reach every target artifact and none of the build script's. Fixing it
//! host-side (`[host] rustflags`, `target-applies-to-host`) is nightly-gated and
//! would have to be got right independently on darwin, debian-glibc and
//! alpine-musl; the only environment where it is even *visible* is the fleet
//! one, which is exactly the R546 trap this ticket exists to close.
//!
//! So the file list is discovered from `cargo metadata` instead: no C++ in the
//! build script, nothing to link, nothing to configure per libc. The precision
//! that was lost is bought back at runtime — `ext_sources::embed_sources` keys
//! off the real `Extension`'s own path and panics if a declared file has no
//! embedded copy, and `every_extension_source_is_runtime_loadable` fails the
//! build for the same reason. A dep bump that adds a file is picked up for free
//! (it is in the crate directory); one that MOVES files out of the crate root
//! fails a test rather than shipping silently.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// The extension crates `ssr::extensions()` registers. Every JS file declared by
/// these is what `JsRuntime::new` would otherwise read off the build machine.
const EXTENSION_CRATES: &[&str] = &["deno_webidl", "deno_web", "deno_net", "deno_fetch"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let js_dir = out_dir.join("ext_js");
    // Stale entries from a previous dep version would otherwise linger and be
    // silently `include_str!`ed by a regenerated table.
    let _ = fs::remove_dir_all(&js_dir);
    fs::create_dir_all(&js_dir).expect("creating OUT_DIR/ext_js");

    // BTreeMap so the generated table is deterministic across builds.
    let mut embedded: BTreeMap<String, PathBuf> = BTreeMap::new();

    for dir in extension_crate_dirs() {
        println!("cargo:rerun-if-changed={}", dir.display());
        for entry in fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("reading extension crate dir {}: {e}", dir.display()))
        {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("js") {
                continue;
            }
            // Top level only. The four crates declare all of their extension JS
            // at the crate root (24 + 8 + 3 + 1 = 36 files at the pinned
            // versions, matching the declared count exactly); the only nested
            // `.js` in the set is `deno_webidl/benches/dict.js`, which is not an
            // extension source.
            let key = source_key(&path);
            let dest = js_dir.join(key.replace('/', "__"));
            fs::copy(&path, &dest).unwrap_or_else(|e| {
                panic!("copying {} -> {}: {e}", path.display(), dest.display())
            });
            println!("cargo:rerun-if-changed={}", path.display());
            embedded.insert(key, dest);
        }
    }

    assert!(
        embedded.len() >= 30,
        "mesofact-ssr build script: only {} extension JS files found across {:?} \
         — the extension crates' layout changed and src/ext_sources.rs would now \
         fail to find sources it must embed",
        embedded.len(),
        EXTENSION_CRATES,
    );

    let mut table = String::from(
        "// @generated by mesofact-ssr/build.rs (R823) — do not edit.\n\
         pub(crate) static EMBEDDED_EXT_SOURCES: &[(&str, &str)] = &[\n",
    );
    for (key, dest) in &embedded {
        table.push_str(&format!(
            "    ({key:?}, include_str!({:?})),\n",
            dest.to_str().expect("OUT_DIR path is not UTF-8"),
        ));
    }
    table.push_str("];\n");

    let table_path = out_dir.join("ext_sources_table.rs");
    fs::write(&table_path, table)
        .unwrap_or_else(|e| panic!("writing {}: {e}", table_path.display()));
}

/// `…/deno_web-0.282.0/06_streams.js` -> `deno_web-0.282.0/06_streams.js`.
///
/// The last two components, not the absolute path: the runtime looks these up
/// from the path `extension!` baked in, and keying on the full string would make
/// the lookup hostage to the two sides canonicalizing symlinks identically. The
/// directory component carries the crate version, so keys stay unique across a
/// graph that somehow contained two versions of the same extension crate.
fn source_key(path: &Path) -> String {
    let file = path.file_name().expect("js file has a name");
    let dir = path
        .parent()
        .and_then(Path::file_name)
        .expect("js file has a parent directory");
    format!("{}/{}", dir.to_string_lossy(), file.to_string_lossy())
}

/// Source directory of each crate in [`EXTENSION_CRATES`], via `cargo metadata`.
///
/// `--offline` is deliberate on the first attempt and does double duty: the
/// dependencies are already downloaded and compiled by the time a build script
/// runs, so it cannot need the network, and it keeps cargo off the exclusive
/// package-cache lock (no downloads to serialize against) — which matters when
/// this compiles inside another cargo invocation. The retry exists only for the
/// case where the lockfile is genuinely out of date locally.
fn extension_crate_dirs() -> Vec<PathBuf> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let run = |offline: bool| {
        let mut cmd = Command::new(&cargo);
        cmd.args(["metadata", "--format-version", "1"]);
        if offline {
            cmd.arg("--offline");
        }
        cmd.output()
    };

    let output = match run(true) {
        Ok(o) if o.status.success() => o,
        _ => run(false).expect("running `cargo metadata`"),
    };
    assert!(
        output.status.success(),
        "mesofact-ssr build script: `cargo metadata` failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let meta: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parsing `cargo metadata` output");
    let packages = meta["packages"]
        .as_array()
        .expect("`cargo metadata` has a packages array");

    let mut dirs = Vec::new();
    for name in EXTENSION_CRATES {
        let mut found = false;
        for pkg in packages {
            if pkg["name"].as_str() != Some(name) {
                continue;
            }
            let manifest = Path::new(
                pkg["manifest_path"]
                    .as_str()
                    .expect("package has a manifest_path"),
            );
            dirs.push(
                manifest
                    .parent()
                    .expect("manifest_path has a parent")
                    .to_path_buf(),
            );
            found = true;
        }
        assert!(
            found,
            "mesofact-ssr build script: `{name}` is not in the dependency graph, \
             but src/ssr.rs registers its extension — the two have drifted",
        );
    }
    dirs
}
