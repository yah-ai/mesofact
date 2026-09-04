//! `mes` — the mesofact dev toolchain.
//!
//! The whole CLI lives here in the library rather than in a bin target,
//! mirroring [`mesofact::cli`]. That is what lets the command ship from a
//! *different package* — `mes`, whose `main.rs` is three lines over
//! [`run`] — while this crate stays a pure library. `mesofact-dev` emits no
//! binaries at all (MFT-R822); see `../Cargo.toml` for why the name you type
//! and the name you depend on are deliberately different.
//!
//! ## Why this is a superset of `mesofact`, not an alias of it
//!
//! `mes` carries the prod verbs (`serve`, `publish`) alongside the dev ones
//! (`dev`, `build`), so a consumer learns one CLI. That does **not** weaken
//! the W225 §2 dev/prod boundary, because that boundary is about which crates
//! land in a *binary*, not about which verbs a binary spells. It only ever
//! runs one direction: this crate already depends on the `mesofact` facade, so
//! `mes serve` is an in-process call into [`mesofact::cli`] — free. The
//! reverse (making the shipped `mesofact` binary a shim over `mes`) would drag
//! the watcher, the dev S3 surface and rolldown into the prod closure, and is
//! exactly what §2 forbids. `mesofact` stays a standalone binary.
//!
//! **Why not spawn the `mesofact` binary for the prod verbs instead of linking
//! it?** (Asked more than once; the answer is not "in-process is tidier".) It
//! would save nothing. `mes dev` — the 95% command — is built directly on the
//! facade's engine: [`mesofact::Server::from_workload`],
//! [`mesofact::ssr::spawn`], [`mesofact::SsrSpawnOptions`],
//! [`mesofact::ProxyMap`]. The facade is linked into this binary for the DEV
//! loop whether or not `serve` is in-process, so process-calling removes zero
//! bytes and adds a runtime dependency on finding `mesofact` on `PATH` —
//! `cargo install mes` would yield a `mes` whose `serve` fails until you
//! separately install `mesofact`. It would also hand us exit-code and
//! signal forwarding for no gain. The link-graph direction is what carries the
//! security property here; the process boundary carries none of it.
//!
//! One verb is absent: **`proxy`** — Mode-2 deployment plumbing (worker pool,
//! manifest reload on SIGHUP). It has no dev-loop meaning; `mesofact proxy`
//! remains its home. Deliberate, and still true.
//!
//! `check` used to be a second one, for a reason that did
//! not hold (R451, reported by @Ashguard:dragon 2026-09-01): the note read
//! "running TypeScript needs node or bun on `PATH`", and it does not —
//! typescript@7 ships `bin/tsc` as a three-line Node shim over a per-platform
//! **native Go binary** (`node_modules/@typescript/typescript-<platform>-<arch>/lib/tsc`)
//! that `mesofact-build`'s `check.rs` resolves and spawns directly.
//!
//! **`mes check` is implemented as of R832-T4**, and the reason it had to be
//! is sharper than "cheap": a scaffolded project's `typecheck` script has to
//! name a binary the user can actually reach, and the R759-F2 trampoline
//! vends exactly three names — `mesofact`, `mes`, `mesofact-dev`. Every other
//! name, `mesofact-build` included, exits 70. So the verb had to appear on one
//! of those three before `mesofact new` could put it in a `package.json`.
//!
//! ## Bare invocation
//!
//! `mes` with no subcommand is `mes dev .` — the 95% command, so it is what
//! you get for typing the least. The one divergence is a workload directory
//! literally named after a subcommand (`mes serve`); spell it `./serve` if
//! that ever comes up.
//!
//! This form also did a second job that is now spent, and the distinction
//! matters because deleting the form along with the job would be wrong. While
//! the binary was still named `mesofact-dev`, the bare form is what made the
//! legacy `mesofact-dev <DIR> --port N …` invocation parse unchanged, so the
//! ~30 spawn sites in the parent camp needed no flag day when `mes` arrived
//! beside it. MFT-R822 retired that binary and migrated those sites, so no
//! caller depends on the bare form for compatibility any more — but it is
//! still the right default for `mes` on its own merits, which is the first
//! paragraph and always was.
//!
//! @yah:relay(R822, "Retire the mesofact-dev BINARY; mes is the only dev command (the package name stays)")
//! @yah:status(review)
//! @yah:at(2026-09-03T08:13:42Z)
//! @yah:assignee(bundle-anthropic-ashguard)
//! @yah:next("OPERATOR DECISION 2026-09-01, and it is the inverse of the split first proposed: descriptive name where you READ it, short name where you TYPE it. The PACKAGE stays mesofact-dev permanently -- not as a legacy spelling of mes, but as the meta crate, the entrypoint to the dev-tier tools mes needs that are NOT mesofact (the watcher, the local S3 surface standing in for R2, serve_app). It depends on the mesofact facade rather than being it, and that direction IS the W225 section 2 prod/dev boundary. The BINARY mesofact-dev goes: nobody wants to type it, ever.")
//! @yah:next("SCOPE, measured not guessed: 30 Rust string-literal sites resolve \"mesofact-dev\" as an executable name (grep '\"mesofact-dev\"' app crates --include=*.rs, excluding @yah: annotation prose, which is the other ~170 hits and is history rather than reference). The load-bearing ones: crates/yah/plugin/src/source.rs:112 keys a builtin plugin manifest on it via include_str!; crates/yah/bundled/src/lib.rs:171 registers it as a bundled binary; app/yah/desktop/src/mesofact_versions.rs:80,94 resolve it through yah_bundled::find and slot.bin(); app/yah/cli/src/plugin_host.rs:414 names it as SourceRef::Bundled.")
//! @yah:next("THE SHARP EDGE, and the reason this is a migration rather than a find-and-replace: ALREADY-INSTALLED store slots have a file literally named mesofact-dev on disk. app/yah/desktop/src/mesofact_versions.rs does slot.bin(\"mesofact-dev\"), so renaming the bin breaks resolution against versions a user already installed. Needs a compat window that accepts either filename, or a store migration -- decide which before touching the 30 sites.")
//! @yah:next("ALSO IN SCOPE, all naming the bin rather than the package: .yah/qed/release-build.toml builds package=mesofact-dev bin=mesofact-dev (-> bin=mes); scripts/check-mesofact-store.sh asserts a slot holds mesofact AND mesofact-dev and that shims exist for mesofact/mes/mesofact-dev; scripts/check-install-sh-compat.sh compares the mesofact-dev shim; oss/mesofact/scripts/check-mesofact-new.sh copies target/debug/mesofact-dev into the test slot and runs `mesofact-dev .`.")
//! @yah:next("AND THE LEGACY-FORM LOGIC ITSELF: cli.rs's bare-invocation design exists specifically so `mesofact-dev <DIR> --port N` parses as `mes dev <DIR>`, which is what let the parent camp's spawn sites keep working without a flag day. Once the bin is gone that rationale is spent -- the bare form is still right for `mes`, but the module doc's justification for it needs rewriting rather than deleting.")
//! @yah:next("NOT BLOCKING THE CRATES.IO PUBLISH. Because the package name is unchanged, mesofact-dev@0.8.29 can be published before any of this lands; removing a bin target later is an ordinary deprecation (cargo install at 0.8.29 gets both binaries, at a later version gets only mes). This ticket was originally framed as racing that publish -- it is not.")
//! @yah:next("END STATE DECIDED BY OPERATOR 2026-09-01, and it supersedes the narrower 'drop the second [[bin]]' framing this ticket opened with: mesofact-dev has NO BIN TARGETS AT ALL. It becomes a pure library -- the meta crate, the entrypoint to the dev-tier tools that are not mesofact. A new thin package `mes` owns the binary: one src/main.rs, three lines over mesofact_dev::cli::run(), depending on mesofact-dev.")
//! @yah:next("WHY THIS SHAPE RATHER THAN JUST DELETING THE mesofact-dev BIN: `cargo install` takes a PACKAGE name, not a bin name. With the binary living in the mesofact-dev package, `cargo install mes` fails with 'could not find mes in registry' and users must know to type `cargo install mesofact-dev` -- unguessable from the command. Splitting gives library named for what it IS and binary named for what you TYPE, and removes the bin-collision footgun of two packages both emitting ~/.cargo/bin/mes.")
//! @yah:next("SAFE TO DO: verified 2026-09-01 that NO crate anywhere takes mesofact-dev as a library dependency today -- the only line in the tree is the library-tier scaffold template (crates/mesofact/src/cli/new/template-lib/Cargo.toml:31), which wants the library and is unaffected. So nothing loses a binary it was consuming, and the new mes package is the first real lib consumer.")
//! @yah:gotcha("SUPERSEDED NOTE, removed rather than left to mislead: an earlier gotcha here said the end state was 'package mesofact-dev while the only [[bin]] is mes'. It is not. The end state is mesofact-dev with ZERO bin targets and a separate `mes` package owning the binary. The cargo package-vs-bin decoupling still matters, but it is now the reason the LIBRARY can keep a descriptive name while the COMMAND gets a short one across a package boundary, not within one.")
//! @yah:gotcha("FEATURE FORWARDING IS THE FIDDLY PART, and this crate already documents the same trap one level down. mesofact-dev's surface is default = [ssr, build]; ssr = [mesofact/ssr, dep:mesofact-publisher, publish]; publish = [mesofact/publish]; build = [mesofact/build]. The new `mes` package must re-expose these (ssr = [mesofact-dev/ssr], etc.) or `cargo install mes --no-default-features` silently loses the lean static/SPA path that exists so consumers can skip the V8 toolchain. mes itself has no cfgs -- cli.rs's #[cfg(feature = ...)] gates evaluate in mesofact-dev -- so forwarding is all that is required, but omitting it is invisible until someone tries the lean build.")
//! @yah:notify_on(R556-F6, "R556-F6 swept MFT-R822 rename fallout you had not reached: oss/qed/crates/qed/images/mesofact-musl-builder/build-mesofact.sh still built `-p mesofact-dev --bin mesofact-dev` and killed a live `yah qed run mesofact-musl` with \"error: no bin target named mesofact-dev in mesofact-dev package\". Fixed there to `-p mes --bin mes`, staged filename now `mes` (install.sh prefers `mes`, keeps `mesofact-dev` only as the declared pre-rename support window). The W225 §2 closure grep was deliberately left alone — `mesofact-dev` is still the right PACKAGE name and only the bin moved. STILL DRIFTING, LEFT FOR YOU because it is the release surface: scripts/publish-mesofact-release.sh (~148-149 cross-build-guarded.sh mesofact-dev, ~171 BINS=(mesofact mesofact-dev mesofact-build)) and .github/workflows/release.yml (~996-1001 --param package=mesofact-dev --param bin=mesofact-dev). Also worth a look: cdn.yah.dev/mesofact/latest.json is 0.8.30 published 2026-09-03T00:38Z and still advertises bins [mesofact, mesofact-dev, mesofact-build] — check whether that publish produced a mesofact-dev binary or whether the manifest now describes a tarball that no longer matches.")
//! @yah:handoff("SPLIT LANDED. New package oss/mesofact/crates/mes — one Cargo.toml, one three-line src/main.rs over mesofact_dev::cli::run(), added to workspace members. mesofact-dev now emits ZERO bin targets: both [[bin]] blocks gone, src/main.rs and src/bin/mesofact-dev.rs deleted, manifest comment rewritten to say why there must never be one again (a bin here re-breaks `cargo install mes`, and a second package emitting `mes` races ../mes for ~/.cargo/bin/mes). cli.rs's module doc no longer claims two bin targets, and its bare-invocation section keeps the form while retiring the spent justification — MFT-R822 migrated the spawn sites, so the bare form now stands on its own merits, which it always did.")
//! @yah:handoff("THE SHARP EDGE, ANSWERED: compat window, not store migration. A slot is a directory a past install.sh untarred and nothing rewrites it, so pre-rename slots hold `mesofact-dev` forever. crates/yah/mesofact-store gains LEGACY_BIN_NAMES + bin_candidates(); Slot::bin resolves the current name through them and falls back to the name asked for, and inspect_slot reports missing under the CURRENT name while accepting the old file. EXPECTED_BINS is now [\"mesofact\", \"mes\"] — a list of current names, which is why it is two constants and not one. mesofact-shim.sh (and its verbatim copy inside install.sh) does the same two-step probe: `$slot/mes`, else `$slot/mesofact-dev`, with the COMPLETENESS check using the resolved path so an old slot reads Complete rather than \"partially installed\". Retiring the fallback is a support-window decision; the shim and LEGACY_BIN_NAMES retire together or not at all, and both say so.")
//! @yah:handoff("PARENT CAMP, all 30 exec-name sites: yah_bundled::BUNDLED's entry is `mes` (that one field is simultaneously the cargo -p, the bin, the staged file and the bundled: source_ref); tauri.conf.json externalBin follows and its drift test passes; desktop mesofact_versions does find(\"mes\") + slot.bin(\"mes\"); .yah/qed/{dashboard-e2e,dashboard-e2e-auth}.toml build and run -p mes --bin mes; scripts/publish-mesofact-release.sh ships BINS=(mesofact mes mesofact-build); .github/workflows/release.yml's leg B, its Package step and both manifest `bins` arrays follow; release-build.toml / mesofact-musl.toml / oss-publish.toml prose corrected. THREE THINGS DELIBERATELY DID NOT MOVE, each for a reason at the site: the plugin ID stays `mesofact-dev` (it keys the PINNED catalog, the discovery index, the Run-tab entry and .yah/jit/mesofact-dev-n — only its source_ref names a binary); MESOFACT_DEV_BIN stays (yubaba's mesofact-static reconciler and `cargo run -p desktop` read it, and renaming a documented env var buys nothing); and `mesofact-dev` stays in SHIM_NAMES as a pure PATH alias onto the same slot binary.")
//! @yah:handoff("THE BUILTIN MANIFEST WAS RE-SIGNED, and that is the one step that needed a secret. source_ref = \"bundled:mesofact-dev\" -> \"bundled:mes\" invalidates the Ed25519 signature, and supervise_plugin verifies BEFORE it spawns and fails closed — so an unsigned edit would have silently stopped the Run tab supervising mesofact-dev at all. Ran the documented ceremony: YAH_PLUGIN_RELEASE_KEY=\"$(yah keys get yah-plugin-release-key)\" cargo run -p xtask -- plugin-sign. The key was never printed and the other three manifests report \"already signed, unchanged\". Proven by desktop::plugins::tests::every_shipped_builtin_verifies_under_the_real_release_key, which is green.")
//! @yah:handoff("DISCOVERED + FIXED, outside the ticket's file list. (1) xtask/src/main.rs check_staged_sidecars was PRESENCE-ONLY, and app/yah/desktop/build.rs deliberately writes a ZERO-BYTE placeholder for any unstaged registry entry so `cargo check -p desktop` passes tauri-build's resource check — so the gate happily passed a staging where the real 82 MB binary sat under the OLD name and a 0-byte `mes-<triple>` sat beside it. `cargo tauri build` would have bundled the empty file. Now a zero-length staged file counts as missing; the fn's doc records the mechanism and how this was found. Verified both ways: red before staging, green after. (2) Staged the real sidecar (cargo run -p xtask -- build-mes-sidecar) and removed the orphaned mesofact-dev-aarch64-apple-darwin, which nothing references any more.")
//! @yah:handoff("THE GOTCHA'S FEATURE FORWARDING IS NOW MECHANICAL, not a promise. mes re-exposes default/ssr/publish/build over mesofact-dev, INCLUDING the ssr->publish implication, and mes/src/main.rs carries a test that parses both manifests and asserts the two [features] tables have identical keys, identical defaults, that every non-default feature forwards `mesofact-dev/<same>`, and that any implication between mesofact-dev's own features is mirrored. It skips silently when the sibling manifest is absent (a packaged .crate). That is the check the gotcha asked for: forgetting a forward is otherwise invisible — mes still builds and silently ignores the flag.")
//! @yah:verify("cargo check --workspace --all-targets (root) -> exit 0. cargo clippy -p yah-mesofact-store -p yah-bundled -p yah-plugin -p xtask --all-targets and -p mes -p mesofact-dev (oss/mesofact) -> exit 0, ZERO warnings in any crate this ticket changed (the ones printed are pre-existing, in yubaba's mesofact_static.rs, xtask/src/install.rs:276, mesofact-core and mesofact-build).")
//! @yah:verify("cargo test — yah-mesofact-store 25 (2 new: a_pre_rename_slot_is_complete_and_resolves_to_the_file_it_holds, the_new_shim_runs_a_pre_rename_slot), yah-bundled 12, yah-plugin 69, xtask --test main bundled 3 (incl. tauri_external_bin_matches_the_bundled_registry), desktop --lib mesofact_versions 5 (1 new: a_slot_installed_before_the_rename_still_resolves) + plugins:: 8, yah --lib plugin_host 11, mesofact-dev 28, mes 1. All 0 failed.")
//! @yah:verify("bash scripts/check-mesofact-store.sh -> 21 passed, 0 failed, driving the REAL embedded install.sh against a local CDN. Two of those cells are new and are the compat window end-to-end: a 0.8.29 tarball whose dev binary is named mesofact-dev installs as a COMPLETE slot, and the CURRENT shim runs it under both `mes` and `mesofact-dev`. bash scripts/dev/run-install-compat-nocosign.sh -> 24 passed, 0 failed, which is what proves the shim re-pasted into install.sh is byte-identical to mesofact-shim.sh.")
//! @yah:verify("Feature forwarding measured, not assumed, via cargo tree -e normal on -p mes: default -> 79 lines matching deno_core|rolldown|mesofact-publisher; --no-default-features -> 0; --no-default-features --features ssr -> deno_core back (9). So the lean static/SPA path that exists to let a consumer skip the V8 toolchain survives the package split.")
//! @yah:gotcha("NOT DONE, AND DELIBERATELY: `mes` is not published to crates.io. The name was unclaimed when this landed (sparse index 404 on /3/m/mes, 2026-09-02) and nothing here reserves it — the first `scripts/oss-publish.sh oss/mesofact` run picks it up on its own, because that script enumerates no members and `cargo publish --workspace` orders by dependency topology, so `mes` lands after `mesofact-dev`. Until then `cargo install mes` still fails for an outside user, which is the very problem this split exists to fix. cdn.yah.dev/mesofact/latest.json is 0.8.30 and advertises bins [mesofact, mesofact-dev, mesofact-build]; that is CORRECT, not drift — it was cut before this landed and its tarball really does hold a file of that name. The compat window covers it; nothing needs republishing, and the next cut ships `mes`.")
//! @yah:gotcha("TWO THINGS LEFT ALONE ON PURPOSE, so nobody re-derives them as omissions. (1) `cargo test --workspace` in oss/mesofact is 194 passed / 3 FAILED, and none of the three are from this change — filed and attributed as R824. They are curated.rs's DELIBERATE red-until-publish alarm firing because the 0.8.31 bump's npm/crates.io publishes have not run (the barrel is still 0.8.29, so it was already red at 0.8.30 too). Do not silence it. (2) ~20 W###/A### docs still contain the string `mesofact-dev`. Checked rather than assumed: every one is either @yah: annotation prose (history, out of scope by the ticket's own framing) or a reference to the PACKAGE / the plugin id / the .mesofact-dev state dir — all three of which are still correct. W225's body needs no edit.")
//! @yah:gotcha("SHARED-TREE NOTE. @Glimmerstone:griffin (R556-F6) fixed oss/qed/crates/qed/images/mesofact-musl-builder/build-mesofact.sh independently on 2026-09-03 after hitting this rename as a LIVE fleet-build failure — `yah qed run mesofact-musl` died ~6 min in with \"no bin target named mesofact-dev in mesofact-dev package\". Their hunks are correct and untouched here, including their decision NOT to change the W225 §2 closure grep (it greps `cargo tree` for the PACKAGE `mesofact-dev`, which is still right — only the bin moved). They also moved the script into mesofact-musl.toml's source_context so the digest-pinned image stops carrying a frozen copy that a rename can silently break. All my edits landed in commit 202fd70a (\"0.8.31\") — a peer's wip-commit swept them in mid-session; nothing was lost, but `git diff` will not show them.")

use std::path::PathBuf;
#[cfg(feature = "ssr")]
use std::sync::Arc;

use clap::{Parser, Subcommand};
#[cfg(feature = "ssr")]
use mesofact::{ssr, SsrSpawnOptions};
use mesofact::{Server, DEFAULT_PORT};
use crate::{watcher, WatchOptions};
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "mes",
    version,
    about = "mes — the mesofact dev toolchain",
    long_about = "Build, serve and ship a mesofact project. `mes` on its own runs the \
                  hot-reload dev loop in the current directory.\n\n\
                  The `serve` and `publish` verbs are the same code the shipped `mesofact` \
                  binary runs; `dev` and `build` are dev-tier and exist only here.",
    // A subcommand and the bare-form args are mutually exclusive: `mes dev .`
    // must not also try to bind `.` to the top-level positional.
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Bare form — `mes [DIR] [--port …]` is `mes dev [DIR] [--port …]`.
    #[command(flatten)]
    dev: DevArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Hot-reload dev loop: build in-process, serve, rebuild on every edit.
    Dev(DevArgs),
    /// One-shot production build into `dist/`. No watcher, no server.
    #[cfg(feature = "build")]
    Build(BuildArgs),
    /// Full TypeScript semantic pass (`tsc --noEmit`) over the project.
    #[cfg(feature = "build")]
    Check(CheckArgs),
    /// Scaffold a new mesofact project pinned to this binary's version.
    New(mesofact::cli::new::NewArgs),
    /// Serve a built bundle or host SSR routes — the prod serving path.
    Serve(mesofact::cli::serve::ServeArgs),
    /// Upload a built dist/ tree, swap the manifest pointer, purge CDN tags.
    #[cfg(feature = "publish")]
    Publish(mesofact::cli::publish::PublishArgs),
}

/// Full semantic pass (`mes check`) — R832-T4, and the verb the header's
/// "check is absent" note said belonged to whichever ticket owns the check
/// surface.
///
/// It matters that this exists on the **dev** binary rather than only on
/// `mesofact-build`. `mesofact-build` ships in the release tarball, but the
/// R759-F2 trampoline dispatches on the name it was invoked as and knows only
/// `mesofact`, `mes` and `mesofact-dev` — every other name exits 70. That is
/// deliberate rather than a gap: `scripts/mesofact-build.sh` reaches that
/// binary by *path* out of a per-version cache, which is how in-repo consumers
/// use it. But a scaffolded `package.json` has only `PATH` to work with, so it
/// can name only a verb the trampoline vends — and `mes check` is one.
///
/// A thin forward to `mesofact::build::check`; this crate already links that
/// crate behind the default-on `build` feature, so it costs a match arm.
#[cfg(feature = "build")]
#[derive(clap::Args, Debug)]
struct CheckArgs {
    /// Project directory containing `tsconfig.json`. Defaults to the current
    /// directory, matching `mes dev`.
    #[arg(default_value = ".")]
    project: PathBuf,

    /// tsconfig path (default: `<project>/tsconfig.json`).
    #[arg(long, value_name = "PATH")]
    tsconfig: Option<PathBuf>,

    /// Extra args forwarded to the checker verbatim, after `--`.
    #[arg(last = true)]
    checker_args: Vec<String>,
}

/// One-shot build (`mes build`).
///
/// Drives [`mesofact::build::pipeline`] directly rather than going through
/// [`crate::Watcher::rebuild`]: `rebuild` additionally snapshots into a
/// `.mesofact-dev/gen-N/` directory and swaps the live pointer, which is the
/// watcher's business. A one-shot build should leave `dist/` and nothing else.
#[cfg(feature = "build")]
#[derive(clap::Args, Debug)]
struct BuildArgs {
    /// Project directory — the parent of `mesofact.routes.ts`.
    #[arg(default_value = ".")]
    workload: PathBuf,

    /// Output directory. Defaults to `<workload>/dist`.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct DevArgs {
    /// Workload directory — the parent of `dist/html/` (e.g. `app/yah/web`).
    /// Defaults to the current directory.
    #[arg(default_value = ".")]
    workload: PathBuf,

    /// TCP port to bind on 127.0.0.1.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Disable the file-watch + auto-rebuild loop; serve whatever's on disk.
    #[arg(long)]
    no_watch: bool,

    /// Skip the initial build at startup (watch mode only).
    #[arg(long)]
    no_initial_build: bool,

    /// Path to a JSON `prefix → backend base URL` map for the same-origin
    /// reverse proxy (R513-F10), e.g.
    /// `{"/auth": "http://127.0.0.1:8745", "/dev": "http://127.0.0.1:8745"}`.
    /// Matching requests are forwarded to the backend before static serving so
    /// the SPA stays single-origin (no CORS). Camp-emitted at SPA-service spawn.
    #[arg(long, value_name = "PATH")]
    proxy_map: Option<PathBuf>,

    /// Path to a JSON file served verbatim at `/config.json` (R513-F5/F10): the
    /// SPA's `DashboardConfig` (apiBaseUrl / authBaseUrl / env …). Injected by
    /// the server — NOT placed in the served `dist/` — so an Option-A pipeline
    /// serving the same bundle never inherits a stale `env:ci` config.
    #[arg(long, value_name = "PATH")]
    config_json: Option<PathBuf>,

    /// Logical service this dev server serves (R602-B4). Paired with
    /// `--component`, it's surfaced at `/__mesofact/info` so the cloud
    /// reconciler can confirm a listener on the configured port is *this*
    /// server before adopting it — refusing a colliding foreign listener
    /// instead of silently hijacking it.
    #[arg(long, requires = "component")]
    service: Option<String>,

    /// Logical component id this dev server serves (R602-B4). See `--service`.
    #[arg(long, requires = "service")]
    component: Option<String>,
}

pub async fn run() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("mesofact_dev=info,tower_http=info")
            }),
        )
        .init();

    let command = match resolve(Cli::parse()) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("mes: {err:#}");
            return std::process::ExitCode::FAILURE;
        }
    };

    match command {
        Command::Dev(args) => to_exit_code(dev(args).await),
        #[cfg(feature = "build")]
        Command::Build(args) => to_exit_code(build(args).await),
        // Mirrors `mesofact-build check`'s exit-code contract rather than
        // to_exit_code's: the checker's own status is the answer, and
        // collapsing every non-zero to 1 would lose it.
        #[cfg(feature = "build")]
        Command::Check(args) => match mesofact::build::check::check(mesofact::build::check::CheckOptions {
            project_root: args.project,
            tsconfig: args.tsconfig,
            extra_args: args.checker_args,
        }) {
            Ok(outcome) if outcome.code == 0 => {
                println!("mes check ok — tsc full semantic pass, no errors");
                std::process::ExitCode::SUCCESS
            }
            // The checker already streamed its diagnostics.
            Ok(outcome) => std::process::ExitCode::from(outcome.code.clamp(1, 255) as u8),
            Err(err) => {
                eprintln!("mes: {err:#}");
                std::process::ExitCode::FAILURE
            }
        },
        Command::New(args) => to_exit_code(mesofact::cli::new::run(args)),
        Command::Serve(args) => to_exit_code(mesofact::cli::serve::run(args).await),
        // `publish` owns its exit codes (2 = missing config, etc.) — pass through.
        #[cfg(feature = "publish")]
        Command::Publish(args) => mesofact::cli::publish::run(args).await,
    }
}

/// Apply the bare-form fallthrough: no subcommand means `dev`.
///
/// Guards the one sharp edge a default subcommand buys. `mes --port 3000 dev`
/// parses as *"run dev against a directory named `dev`"* — clap fills the
/// positional because the leading flag already moved the parser past the
/// subcommand slot. Silently serving the wrong directory is a poor answer to
/// an obvious word-order typo, so name it instead. The subcommand list comes
/// from clap rather than a literal, so a verb added above cannot forget to
/// appear here.
fn resolve(cli: Cli) -> anyhow::Result<Command> {
    use clap::CommandFactory;

    if let Some(command) = cli.command {
        return Ok(command);
    }
    if let Some(name) = Cli::command()
        .get_subcommands()
        .map(clap::Command::get_name)
        .find(|name| std::path::Path::new(name) == cli.dev.workload)
    {
        anyhow::bail!(
            "`{name}` is a subcommand, but a flag came first so it was read as a \
             directory. Put the subcommand first: `mes {name} …` \
             (or say `./{name}` if you did mean the directory)."
        );
    }
    Ok(Command::Dev(cli.dev))
}

fn to_exit_code(result: anyhow::Result<()>) -> std::process::ExitCode {
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mes: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// `mes build` — run the pipeline once and stop.
#[cfg(feature = "build")]
async fn build(args: BuildArgs) -> anyhow::Result<()> {
    let project_root = args
        .workload
        .canonicalize()
        .unwrap_or_else(|_| args.workload.clone());
    let out_dir = args.out_dir.unwrap_or_else(|| project_root.join("dist"));
    info!(root = %project_root.display(), out = %out_dir.display(), "mes build");
    mesofact::build::pipeline::build(mesofact::build::pipeline::BuildOptions {
        project_root,
        out_dir: Some(out_dir),
        build_id: None,
        // Same `Auto` as the watcher: materialize `node_modules` from the
        // lockfile only when missing, so the build needs no package manager.
        install: mesofact::build::pipeline::InstallMode::Auto,
    })
    .await?;
    Ok(())
}

/// `mes dev` — the hot-reload loop. This is the whole of what the
/// `mesofact-dev` binary used to be.
async fn dev(args: DevArgs) -> anyhow::Result<()> {
    let mut server = Server::from_workload(&args.workload)?;

    // Logical identity (R602-B4): stamp the server with the (service, component)
    // the reconciler spawned it for so `/__mesofact/info` lets the adopt path
    // verify the port holds *this* server. clap's `requires` couples the pair,
    // so both-or-neither is guaranteed here.
    if let (Some(service), Some(component)) = (&args.service, &args.component) {
        info!(service, component, "mesofact-dev: identity stamped (R602-B4)");
        server = server.with_identity(service.clone(), component.clone());
    }

    // Same-origin reverse proxy (R513-F10): forward `/auth/*` etc. to the
    // camp-vended backend ports so the dashboard E2E (Option B) browser stays
    // single-origin. No map → no proxy (the Option A static path is unchanged).
    if let Some(map_path) = &args.proxy_map {
        let map = mesofact::ProxyMap::from_json_file(map_path)?;
        info!(
            map = %map_path.display(),
            routes = ?map.routes(),
            "mesofact-dev: same-origin reverse proxy installed",
        );
        server = server.with_proxy(map);
    }

    // Server-injected runtime config (R513-F5/F10) served at /config.json.
    if let Some(config_path) = &args.config_json {
        let bytes = std::fs::read(config_path)
            .map_err(|e| anyhow::anyhow!("reading config json {}: {e}", config_path.display()))?;
        info!(config = %config_path.display(), "mesofact-dev: serving /config.json (R513-F10)");
        server = server.with_config_json(bytes);
    }

    // Canonicalize so the bun child's manifest read + dynamic-import use
    // absolute paths regardless of the cwd mesofact-dev was invoked from.
    let workload_abs = args
        .workload
        .canonicalize()
        .unwrap_or_else(|_| args.workload.clone());
    let state_dir = workload_abs.join(".mesofact-dev");
    #[cfg(feature = "ssr")]
    let ssr_slot = server.ssr_slot();

    // Dev-tier S3 surface (R490-F7): host a local s3s-fs bucket so a workload's
    // @mesofact/runtime R2Adapter can resolve against it during dev instead of
    // real Cloudflare R2. Coords go to .mesofact-dev/s3.json for discovery,
    // into the build child's env below, AND (R444) into the in-process SSR
    // isolate's env — started before the SSR spawn below so the first boot
    // already has coordinates, not just post-build respawns.
    let dev_s3 = crate::DevS3::start(state_dir.join("s3"), crate::DEV_S3_BUCKET).await?;
    info!(endpoint = %dev_s3.endpoint, bucket = %dev_s3.bucket, "dev S3 surface ready");

    // Attach an SSR child if the workload's manifest declares any mode:"ssr"
    // routes. ssr::spawn returns Ok(None) for static/SPA-only workloads (or
    // when no build has emitted a manifest yet); the no-bun path is preserved
    // and the post-build hook below retries lazily. (Compiled out entirely
    // under --no-default-features: static/SPA/proxy serving without V8.)
    #[cfg(feature = "ssr")]
    {
        let ssr_opts = SsrSpawnOptions::new(
            workload_abs.clone(),
            workload_abs.join("dist"),
            state_dir.clone(),
        )
        .with_env(dev_s3.env_vars());
        match ssr::spawn(ssr_opts).await? {
            Some(child) => {
                info!(prefixes = ?child.prefixes(), "mesofact-dev ssr child attached");
                ssr_slot.set(Some(Arc::new(child)));
            }
            None => {
                info!("mesofact-dev: no SSR routes (or no manifest yet); static-only");
            }
        }
    }

    // Instance-addressed (deferred) route resolution (W270 §9): point an
    // S3Store at the dev-S3 surface so a static miss on a `prerender:
    // { deferred: true }` route resolves through the pointer store against the
    // same local bucket the publisher flips into — the local mirror of the edge
    // worker's R2 resolution. Region "auto" + dummy creds match R2 / the
    // anonymous dev surface (s3s skips signature verification).
    #[cfg(feature = "ssr")]
    {
        use mesofact_publisher::{ObjectStore, S3Store};
        match S3Store::new(dev_s3.endpoint.clone(), dev_s3.bucket.clone(), "auto", "dev", "dev") {
            Ok(store) => {
                server = server.with_instance_store(Arc::new(store) as Arc<dyn ObjectStore>);
                info!("mesofact-dev: instance-addressed route resolution wired to dev S3 (W270 §9)");
            }
            Err(e) => {
                info!(error = %e, "mesofact-dev: could not build dev pointer store; deferred routes 404")
            }
        }
    }

    if let Err(e) = std::fs::write(
        state_dir.join("s3.json"),
        serde_json::json!({ "endpoint": dev_s3.endpoint, "bucket": dev_s3.bucket }).to_string(),
    ) {
        info!(error = %e, "dev S3: could not write s3.json discovery file");
    }

    if args.no_watch {
        info!("watch mode disabled");
        return server.serve(args.port).await;
    }

    let pointer = server.pointer();
    // `workload_abs`, not `args.workload` (R759-T4). The watcher's paths flow
    // into the post-build hook's `gen_dir`, and the SSR pool registers each
    // entrypoint as a `file://` module URL — which cannot be built from a
    // relative path. Running `mesofact-dev .` (the invocation the scaffold's
    // README gives, and the natural one from inside a project) therefore lost
    // EVERY `mode: "ssr"` route: the initial spawn logs "no SSR routes" because
    // no manifest exists yet, the post-build respawn then fails, and the only
    // trace is one WARN about a "publish hook" that has nothing to do with
    // publishing. Routes 404 as if they were never declared.
    let mut opts = WatchOptions::defaults_for_workload(&workload_abs);
    opts.initial_build = !args.no_initial_build;
    opts.build_env = dev_s3.env_vars();

    let watcher_obj = crate::Watcher::new(workload_abs.clone(), pointer, opts);

    // Post-build hook: each successful rebuild rotates dist into
    // .mesofact-dev/gen-N/, so the SSR runtime must be re-spawned against
    // the new gen dir — V8's module cache would otherwise keep serving the
    // old route entrypoints. Under R449-F2 the in-process model swaps the
    // whole SsrChild in the slot (no SIGKILL/respawn dance the bun era
    // needed); the prior Arc<SsrChild> drops, which joins the isolate
    // thread.
    #[cfg(feature = "ssr")]
    let watcher_obj = {
        let slot_for_hook = ssr_slot.clone();
        let workload_for_hook = workload_abs.clone();
        let state_dir_for_hook = state_dir.clone();
        let dev_s3_for_hook = dev_s3.clone();
        let hook: watcher::PostBuildFn = Box::new(move |gen_dir: PathBuf| {
            let slot = slot_for_hook.clone();
            let workload = workload_for_hook.clone();
            let state_dir = state_dir_for_hook.clone();
            let env = dev_s3_for_hook.env_vars();
            Box::pin(async move {
                let opts = SsrSpawnOptions::new(workload, gen_dir, state_dir).with_env(env);
                match ssr::spawn(opts).await? {
                    Some(child) => {
                        info!(
                            prefixes = ?child.prefixes(),
                            "mesofact-dev ssr runtime re-spawned against new gen",
                        );
                        slot.set(Some(Arc::new(child)));
                    }
                    None => {
                        // Manifest declares no SSR routes; clear any prior child.
                        slot.set(None);
                    }
                }
                Ok(())
            })
        });
        watcher_obj.with_post_build(hook)
    };

    let watcher_task = watcher::spawn(watcher_obj);

    // Server owns the foreground; the spawned watcher continues until the
    // process exits.
    let result = server.serve(args.port).await;
    drop(watcher_task);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(argv: &[&str]) -> Cli {
        Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?} failed to parse:\n{e}"))
    }

    /// Resolve exactly as `main` does, so these tests exercise the real
    /// bare-form fallthrough rather than a restatement of it.
    fn dispatch(argv: &[&str]) -> Command {
        resolve(parse(argv)).unwrap_or_else(|e| panic!("{argv:?} did not resolve:\n{e}"))
    }

    #[test]
    fn clap_definition_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// THE compatibility contract, and it outlived the binary it was written
    /// for. Every dev-server spawn site in the parent camp (kamaji, desktop,
    /// `serve_build.rs`, `local.sh`, …) passes this argv SHAPE — a bare
    /// directory followed by `--port`/`--no-watch`/`--service`/`--component`,
    /// no subcommand. MFT-R822 renamed the executable those sites exec from
    /// `mesofact-dev` to `mes`; argv[0] is the only thing that moved, which is
    /// why this test is spelled with the new name and asserts the same
    /// bindings. A failure here breaks every one of those sites at once.
    #[test]
    fn bare_directory_spawn_shape_still_parses() {
        let Command::Dev(args) = dispatch(&[
            "mes",
            "app/yah/web",
            "--port",
            "8080",
            "--no-watch",
            "--service",
            "dashboard",
            "--component",
            "web",
        ]) else {
            panic!("bare-directory spawn shape did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("app/yah/web"));
        assert_eq!(args.port, 8080);
        assert!(args.no_watch);
        assert_eq!(args.service.as_deref(), Some("dashboard"));
        assert_eq!(args.component.as_deref(), Some("web"));
    }

    #[test]
    fn bare_mes_is_dev_in_the_current_directory() {
        let Command::Dev(args) = dispatch(&["mes"]) else {
            panic!("bare `mes` did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("."));
        assert_eq!(args.port, DEFAULT_PORT);
    }

    /// `mes check` has to be a real subcommand rather than a directory named
    /// `check` handed to the bare `dev` form — the bare-form fallthrough makes
    /// that a live confusion, and a scaffolded `package.json` names this verb.
    #[cfg(feature = "build")]
    #[test]
    fn check_is_a_subcommand_and_defaults_to_here() {
        let Command::Check(args) = dispatch(&["mes", "check"]) else {
            panic!("`mes check` did not resolve to the check verb");
        };
        assert_eq!(args.project, PathBuf::from("."));
        let Command::Check(args) = dispatch(&["mes", "check", "site"]) else {
            panic!("`mes check site` did not resolve to the check verb");
        };
        assert_eq!(args.project, PathBuf::from("site"));
    }

    #[test]
    fn bare_form_takes_dev_flags() {
        let Command::Dev(args) = dispatch(&["mes", "site", "--port", "3000"]) else {
            panic!("bare form with flags did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("site"));
        assert_eq!(args.port, 3000);
    }

    #[test]
    fn explicit_dev_subcommand_binds_its_own_args() {
        let Command::Dev(args) = dispatch(&["mes", "dev", "site", "--port", "3000"]) else {
            panic!("`mes dev` did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("site"));
        assert_eq!(args.port, 3000);
    }

    /// A leading flag pushes clap past the subcommand slot, so `dev` lands on
    /// the positional. Serving a directory named `dev` is not what anyone
    /// typing this meant — `resolve` has to catch it.
    #[test]
    fn subcommand_after_a_flag_is_rejected_not_silently_served() {
        let cli = parse(&["mes", "--port", "3000", "dev"]);
        assert!(cli.command.is_none(), "clap bound `dev` as a subcommand after all");
        assert_eq!(cli.dev.workload, PathBuf::from("dev"));

        let err = resolve(cli).expect_err("`mes --port 3000 dev` should not resolve").to_string();
        assert!(err.contains("`dev` is a subcommand"), "unhelpful message: {err}");
        assert!(err.contains("mes dev"), "message omits the fix: {err}");
    }

    /// The guard keys off the subcommand names, so a real directory that
    /// merely starts the same way must still go through.
    #[test]
    fn guard_does_not_swallow_ordinary_directories() {
        let Command::Dev(args) = dispatch(&["mes", "developer-site"]) else {
            panic!("`developer-site` was not treated as a workload directory");
        };
        assert_eq!(args.workload, PathBuf::from("developer-site"));

        // And the escape hatch the error message promises actually works.
        let Command::Dev(args) = dispatch(&["mes", "./dev"]) else {
            panic!("`./dev` was not treated as a workload directory");
        };
        assert_eq!(args.workload, PathBuf::from("./dev"));
    }

    #[test]
    #[cfg(feature = "build")]
    fn build_defaults_to_cwd_and_takes_out_dir() {
        let Command::Build(args) = dispatch(&["mes", "build"]) else {
            panic!("`mes build` did not resolve to `build`");
        };
        assert_eq!(args.workload, PathBuf::from("."));
        assert_eq!(args.out_dir, None);
    }

    /// The prod verbs are reachable from `mes` — that is the whole point of
    /// the superset, and it is the half a `cargo check` cannot confirm.
    #[test]
    fn prod_verbs_are_reachable() {
        assert!(matches!(dispatch(&["mes", "serve"]), Command::Serve(_)));
        assert!(matches!(dispatch(&["mes", "new", "hello"]), Command::New(_)));
        #[cfg(feature = "publish")]
        assert!(matches!(dispatch(&["mes", "publish"]), Command::Publish(_)));
    }

    /// `--lib` (R832-T2) reaches `mesofact::cli::new` through this CLI too.
    /// The flag is defined on the facade's `NewArgs`, so a `mes new --lib`
    /// that failed to parse would mean the two CLIs had drifted apart — the
    /// exact thing the superset exists to prevent.
    #[test]
    fn new_takes_the_library_tier_flag() {
        let Command::New(args) = dispatch(&["mes", "new", "--lib", "hello"]) else {
            panic!("expected New");
        };
        assert!(args.lib);
        assert_eq!(args.path, PathBuf::from("hello"));

        let Command::New(args) = dispatch(&["mes", "new", "hello"]) else {
            panic!("expected New");
        };
        assert!(!args.lib, "the standalone tier stays the default");
    }
}
