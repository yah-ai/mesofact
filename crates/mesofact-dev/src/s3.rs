//! Dev-tier S3 store resolution (R584-T1, partly reversed 2026-09-17).
//!
//! A workload's `@mesofact/runtime` `R2Adapter` needs a *local* object store to
//! resolve against during the dev loop, instead of real Cloudflare R2. There
//! are two providers, and [`DevStore::resolve`] picks between them:
//!
//! - **The camp's.** A running `yah camp` supervises `yah-s3-fs` (W265) and
//!   injects its coordinates into every dev service's environment (R274-F5,
//!   `cloud::reconciler::s3_driver`). Preferred whenever present: its objects
//!   are shared with every other app in the camp and outlive this process,
//!   where an embedded store's are visible to nothing else.
//! - **An embedded [s3s-fs] surface** under `.mesofact-dev/s3/`, started here.
//!
//! R584-T1 deleted the embedded arm, on the premise that the camp is always
//! there. That premise holds inside a camp and nowhere else — mesofact ships to
//! people who have no yah installed, and `mes .` on a freshly scaffolded
//! project is the invocation the scaffold's own README gives. The standalone
//! tier of `check-mesofact-new.sh` runs exactly that, with no camp, and had
//! been failing on it for six days when this arm was restored (2026-09-17,
//! blocking the 0.8.40 release wave at `release-check`).
//!
//! This is not the fallback-beside-the-real-thing CLAUDE.md warns about: both
//! arms are load-bearing product behaviour, neither is a compatibility shim for
//! the other, and a *half-set* camp injection is still a hard error rather than
//! a third path — see [`DevStore::resolve`].
//!
//! [s3s-fs]: https://docs.rs/s3s-fs
//!
//! @yah:ticket(R584-T1, "Retire mesofact-dev's private in-process s3s-fs surface in favour of the camp s3 driver")
//! @yah:at(2026-09-12T20:41:31Z)
//! @yah:status(review)
//! @yah:parent(R584)
//! @yah:handoff("Deleted the whole private in-process s3s-fs surface: DevS3::start, build_service, AllowAllAccess, serve_loop, and the TcpListener/hyper_util/s3s/s3s-fs/async-trait code all gone from crates/mesofact-dev/src/s3.rs (was 208 lines, now ~145). DevS3 is now a plain coordinate struct with a from_env() constructor that reads S3_ENDPOINT/S3_BUCKET/S3_ACCESS_KEY_ID/S3_SECRET_ACCESS_KEY and keeps its env_vars() method (still emits R2_ENDPOINT/R2_BUCKET/R2_ACCESS_KEY_ID/R2_SECRET_ACCESS_KEY for workload consumers, unchanged shape/names).")
//! @yah:handoff("Wired from_env() into all six injection sites: cli.rs:410 (was DevS3::start(state_dir.join(\"s3\"), DEV_S3_BUCKET).await?), cli.rs:425/487/508 (.env_vars() call sites unchanged, now sourced from real fields), cli.rs:446-452 (S3Store::new now uses dev_s3.access_key_id/secret_access_key instead of the old hardcoded \"dev\"/\"dev\"), app.rs:104 (DevServer::start), app.rs's export_env loop unchanged (already generic over env_vars()).")
//! @yah:handoff("Deleted DEFAULT_BUCKET/DEV_S3_BUCKET entirely (dead once bucket comes from the camp, not minted) — removed the re-export from lib.rs and all four call sites that passed it as a start() arg.")
//! @yah:handoff("Cargo.toml: dropped s3s, s3s-fs, async-trait, hyper-util, reqwest (all four were only used by the deleted server/round-trip-test code; grepped clean afterward). Kept tower/tower-http/mesofact-publisher — unrelated to this surface.")
//! @yah:handoff("Standalone error (point 4): DevS3::from_env() bails naming exactly which of the four vars is missing plus 'yah camp' as the fix — see s3.rs's error string. Partial-set is a hard error (never fills a default), matching the ticket's decision.")
//! @yah:handoff("Independent verification (MFT-R584-T1) confirmed DevS3 was a thin env-reader with zero surviving server/spawn behaviour, so it was renamed to CampS3 per CLAUDE.md's 'rename to what it is, no aliases' -- every in-crate call site (s3.rs, app.rs, lib.rs, cli.rs) and the three live doc-comment mentions outside the crate (crates/mesofact/src/server.rs, crates/mesofact/src/ssr.rs, crates/mesofact-ssr/src/ssr.rs) updated; historical @yah: annotations describing past DevS3-named state left untouched. Re-verified: build (both feature sets)/test/clippy all still clean after the rename.")
//! @yah:handoff("Test fallout, expected and unavoidable: 5 tests deleted because they drove the now-deleted in-process server end-to-end (s3.rs's two round-trip tests, lib.rs's two cross-boundary SSR/deferred-route smokes, app.rs's handler_reads_r2_from_env test) — none of that integration behavior is testable from this crate anymore since the server moved to yah-s3-fs (out of this crate's tree). Replaced with 5 new tests covering the actual surface: s3::tests::from_env_reads_camp_injected_coordinates, s3::tests::from_env_errors_naming_missing_vars_and_yah_camp, app::tests::start_reads_camp_coordinates_and_writes_discovery_file (rewritten from start_creates_state_dir_and_discovery_file), app::tests::start_errors_naming_yah_camp_when_nothing_injected (new), app::tests::export_env_publishes_r2_coordinates_from_camp_env (new). Net 28 -> 27 tests.")
//! @yah:handoff("Added lib.rs's #[cfg(test)] test_support::ENV_LOCK (tokio::sync::Mutex<()>) shared by s3::tests and app::tests since both mutate the same process-wide S3_*/R2_* env vars — a lock private to one module doesn't stop the parallel test runner racing the other module. Used tokio::sync::Mutex specifically (not std::sync::Mutex) because app::tests holds the guard across DevServer::start's .await; std::sync::Mutex there was clippy::await_holding_lock at baseline-clean crate.")
//! @yah:verify("Baseline (measured before any edit): cargo test -p mesofact-dev = 28 passed, 0 failed (28/0). cargo build -p mesofact-dev clean. cargo clippy -p mesofact-dev --all-targets: zero warnings attributed to mesofact-dev itself (all warnings in that run belong to other workspace crates: mesofact-core, mesofact-build, rnpm).")
//! @yah:verify("After: cargo build -p mesofact-dev clean (both default features and --no-default-features). cargo test -p mesofact-dev = 27 passed, 0 failed (27/0) -- net -1 from 5 deletions + 4 additions per the handoff note above (doc-tests unaffected, 0 passed/2 ignored both times). cargo clippy -p mesofact-dev --all-targets: zero warnings attributed to mesofact-dev (same other-crate warnings as baseline, nothing new).")
//! @yah:gotcha("cli.rs and Cargo.toml landed in a camp wip-commit (HEAD 269486ae \"sync\") partway through this session while s3.rs/app.rs/lib.rs were still uncommitted in the working tree -- both states carry the same content (verified by `git show 269486ae:oss/mesofact/crates/mesofact-dev/src/cli.rs`), so nothing was lost, but don't be surprised if `git status` under-reports which files this ticket touched.")
//! @yah:gotcha("Scope-fence held: everything landed inside oss/mesofact/crates/mesofact-dev/. Did NOT touch crates/mesofact/src/cli/new/template-lib/src/bin/__PROJECT_NAME__-dev.rs (an `ignore`-tagged doc example referencing DevServer::start(\".\")) or crates/mesofact/src/ssr.rs's doc comments mentioning DevS3::env_vars() -- both are outside the crate and both are still accurate (DevServer::start and env_vars() keep their old names/signatures precisely so those out-of-crate references don't need editing).")
//! @yah:handoff("SCOPE EXTRA, deliberate and leader-authorized: DevS3 was renamed to CampS3. The ticket said \"delete DevS3\", and the implementer's first pass instead kept the struct as a thin env-reader — correct behaviour, misleading name. The verification pass confirmed zero server/spawn code remained in it and then landed the rename across every in-crate call site plus three live cross-crate doc comments in mesofact / mesofact-ssr that still described the type as minting a store. Per CLAUDE.md's \"rename to what the thing actually is, fix every call site, no aliases\" — no alias was left behind. Build / test / clippy were re-run AFTER the rename and are unchanged (27/0, clean, clean); the mesofact and mesofact-ssr compiles in that re-run also cover the touched doc comments. Historical @yah: annotations still say DevS3 on purpose — they are a record of what was there.")
//! @yah:verify("Fallback audit came back negative by grep, which is the finding that actually matters here: no surviving server bind, no s3s reference, no S3Store spawn, and no hardcoded \"dev\" bucket-or-credential literal used as a RUNTIME default anywhere in the crate. The remaining \"dev\" literals are test fixtures. Dropped deps confirmed unreferenced including under #[cfg(test)] and behind both feature gates; they are absent from mesofact-dev's own entry in oss/mesofact/Cargo.lock and survive there only as transitive deps of other crates, with `cargo metadata` exit 0 plus the four green builds as the consistency evidence.")
//! @yah:gotcha("NOTHING IS COMMITTED. Camp git policy is defer and this is a shared working tree; the edits sit uncommitted in oss/mesofact/crates/mesofact-dev/ (s3.rs, app.rs, cli.rs, lib.rs, Cargo.toml), oss/mesofact/Cargo.lock, and the three renamed doc-comment sites in mesofact / mesofact-ssr. Whoever sweeps git should note that `git add` takes whole files and this tree carries other sessions' uncommitted work.")
//! @yah:verify("Standalone-case error was exercised by hand, not just read: the `mes` binary run with all four of S3_ENDPOINT/S3_BUCKET/S3_ACCESS_KEY_ID/S3_SECRET_ACCESS_KEY unset, and again with only two of four set, both exit 1 with a clear message naming exactly which vars are missing and naming `yah camp` as the thing that supplies the store — no panic, no backtrace, no hang, and the partial case does not silently guess a default. Test count went 28/0 -> 27/0: five tests that seeded data through the now-deleted in-process server were removed and replaced with env-based ones; the server behaviour they covered did not lose coverage, it MOVED — yah-s3-fs now carries 26 router/store/sigv4/policy tests for it. Two independent passes: an implementing courier and a separate adversarial verification courier that re-ran every command itself rather than reading the first one's report.")
//! @yah:next("OPEN DESIGN QUESTION the operator raised while this landed, not actioned here: should the dev-tier store be a plugin rather than compiled into mes? Two halves. (a) CAMP ARM — yah-s3-fs is already a supervised out-of-process binary (cloud::reconciler::s3_driver spawns it, kamaji assigns the mesh port, it publishes coords.json), so promoting it to a W232 plugin under app/yah/cli/src/plugin_host.rs would buy a signed manifest, a declared source_ref and an enforced Requires grant set, which the hardcoded best-effort activate has none of. The same argument covers the pg and smtp drivers — one cleanup, not three. (b) STANDALONE ARM — a camp plugin cannot cover this, since a plugin presupposes a camp and mesofact ships to people with no yah installed; the operator's framing was that mes either carries the store itself (what landed) or grows its OWN plugin system. DevStore::resolve is the seam either would plug into.")
//! @yah:handoff("THE FIX, and what was NOT reverted: CampS3::from_env() became DevStore::resolve(state_dir) (async) in crates/mesofact-dev/src/s3.rs, with a StoreProvenance::{Camp,Embedded} field recording which provider answered. Three cases: all four S3_* set = use the camp's store (preferred — its objects are shared camp-wide and outlive the process, which is T1's actual argument and it is still right); NONE set = start the embedded s3s-fs surface under .mesofact-dev/s3/, restored from 5896a06f including AllowAllAccess + SimpleAuth(dev/dev) so SigV4 clients still verify; SOME set = hard error naming both the present and the missing vars, because a half-set injection is a miswired camp and quietly starting a second empty store would hide it behind a dev loop that looks fine until a published object turns up missing. That partial-set refusal is T1's decision and it is KEPT. Deps s3s 0.13 / s3s-fs 0.13 / async-trait / hyper-util came back to mesofact-dev's Cargo.toml; reqwest did not (it only served the deleted round-trip tests). CampS3 renamed to DevStore at every call site (lib.rs, app.rs, cli.rs) plus the three cross-crate doc mentions T1 itself had updated (mesofact/src/server.rs, mesofact/src/ssr.rs, mesofact-ssr/src/ssr.rs) — no alias left behind, per CLAUDE.md.")
//! @yah:verify("cargo test -p mesofact-dev = 29 passed / 0 failed (T1's stated baseline was 27/0). cargo build -p mesofact-dev clean under BOTH default features and --no-default-features. cargo clippy -p mesofact-dev --all-targets: zero warnings attributed to mesofact-dev/src, checked by grepping the log for that path rather than eyeballing a count. THE DECISIVE ONE: bash scripts/check-mesofact-new.sh run end to end = PASSED, 41 passed / 0 failed / 1 skipped, against the same script that had just failed the release. The 1 skip is the library tier declining because the scaffold pins mesofact 0.8.40 and crates.io is still at 0.8.37 — expected pre-publish, not a masked failure. Tests added: s3::tests::resolve_starts_an_embedded_store_when_no_camp_injected_anything (asserts the listener ACCEPTS a TCP connect before resolve returns, not merely that the endpoint string is well-formed), s3::tests::resolve_refuses_a_half_set_injection_instead_of_falling_back, app::tests::start_comes_up_on_an_embedded_store_when_nothing_is_injected (replaces start_errors_naming_yah_camp_when_nothing_injected, which asserted exactly the contract that broke the gate), app::tests::start_errors_on_a_half_set_injection.")
//! @yah:gotcha("PARTLY REVERSED 2026-09-17, operator call, after it broke the release gate for six days. T1's premise was that a running `yah camp` always injects S3_*; that holds inside a camp and nowhere else. `mes .` on a freshly scaffolded project — the invocation the scaffold's own README gives, and what the standalone tier of oss/mesofact/scripts/check-mesofact-new.sh runs — has no camp, so every such run died on T1's own error string. mesofact-new-smoke last passed 2026-09-11; T1 landed 2026-09-12; the next run of that gate (2026-09-17, then again inside release wizard runs f848936d and ebf4978a for 0.8.40) failed at check-mesofact-new with `mes never served /`. It blocked the 0.8.40 wave at release-check, the last reversible point before oss-publish.")

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tokio::net::TcpListener;

/// Env vars a running `yah camp` injects for this workspace's dev-tier S3
/// driver (R274-F5, `cloud::reconciler::s3_driver`). Order matches the
/// missing-var listing in [`DevStore::resolve`]'s error.
const REQUIRED_ENV: [&str; 4] = [
    "S3_ENDPOINT",
    "S3_BUCKET",
    "S3_ACCESS_KEY_ID",
    "S3_SECRET_ACCESS_KEY",
];

/// Bucket the embedded surface pre-creates. Workloads point `[sources.r2]
/// bucket` here when running outside a camp.
pub const EMBEDDED_BUCKET: &str = "dev";

/// Credentials the embedded surface accepts. SigV4-signing clients (the
/// publisher's `S3Store`, the JS `R2Adapter`) sign with these; s3s verifies the
/// signature against them.
const EMBEDDED_ACCESS_KEY: &str = "dev";
const EMBEDDED_SECRET_KEY: &str = "dev";

/// Where the coordinates in a [`DevStore`] came from.
///
/// Recorded rather than inferred because the two provenances have genuinely
/// different operational meaning: `Camp` objects outlive the process and are
/// shared with every other app in the camp, `Embedded` ones live under this
/// project's `.mesofact-dev/` and die with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreProvenance {
    /// A running `yah camp` injected `S3_*` into this process's environment.
    Camp,
    /// No camp injected anything, so this process started its own surface.
    Embedded,
}

/// Coordinates of the dev object store this process talks to, and where they
/// came from. Handed to consumers (build-child env, discovery file, the
/// in-process V8 SSR runtime) so they can point an S3 client at it.
#[derive(Debug, Clone)]
pub struct DevStore {
    /// e.g. `http://127.0.0.1:54321` — no trailing slash, path-style.
    pub endpoint: String,
    /// The dev bucket: the camp's per-workspace one, or [`EMBEDDED_BUCKET`].
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Which of the two providers supplied the fields above.
    pub provenance: StoreProvenance,
}

impl DevStore {
    /// Resolve the dev object store, preferring the camp's and falling back to
    /// an embedded one rooted at `<state_dir>/s3`.
    ///
    /// Three cases, and the middle one is the reason this is not a plain
    /// `unwrap_or_else`:
    ///
    /// - **All four `S3_*` set** — a `yah camp` injected them. Use the camp's
    ///   store, so `mes` and every other app in the camp share one bucket.
    /// - **Some but not all set** — a configuration error, not a fallback
    ///   signal. Somebody meant to use the camp's store and got it half-wired;
    ///   silently starting a second, empty store would hide that behind a dev
    ///   loop that looks fine until a published object is missing. Bails naming
    ///   exactly which vars are absent.
    /// - **None set** — standalone (`mes .` on a scaffolded project, the
    ///   invocation the scaffold's README gives). Start the embedded surface.
    ///
    /// R584-T1 deleted the embedded arm on the premise that a camp is always
    /// there; it was restored 2026-09-17 after the standalone tier of
    /// `check-mesofact-new.sh` — which runs `mes` with no camp at all — failed
    /// on exactly that premise for six days.
    pub async fn resolve(state_dir: impl AsRef<Path>) -> Result<DevStore> {
        let present: Vec<&str> = REQUIRED_ENV
            .iter()
            .filter(|var| std::env::var(**var).is_ok())
            .copied()
            .collect();

        if present.len() == REQUIRED_ENV.len() {
            return Ok(DevStore {
                endpoint: std::env::var("S3_ENDPOINT").expect("checked above"),
                bucket: std::env::var("S3_BUCKET").expect("checked above"),
                access_key_id: std::env::var("S3_ACCESS_KEY_ID").expect("checked above"),
                secret_access_key: std::env::var("S3_SECRET_ACCESS_KEY").expect("checked above"),
                provenance: StoreProvenance::Camp,
            });
        }

        if !present.is_empty() {
            let missing: Vec<&str> = REQUIRED_ENV
                .iter()
                .filter(|var| std::env::var(**var).is_err())
                .copied()
                .collect();
            bail!(
                "dev-tier S3 coordinates are half-set: {} present, {} missing. A partial set \
                 means a `yah camp` injection went wrong, so mesofact-dev refuses to guess — \
                 it will not quietly start its own empty store and hide the miswiring. Either \
                 set all four, or unset {} and let mesofact-dev start an embedded store.",
                present.join(", "),
                missing.join(", "),
                present.join(", "),
            );
        }

        Self::start_embedded(state_dir.as_ref().join("s3")).await
    }

    /// Start the embedded surface: create `<root>/<bucket>/`, bind
    /// `127.0.0.1:0`, and spawn the serve loop on the current tokio runtime.
    /// The server runs until the process exits.
    async fn start_embedded(root: PathBuf) -> Result<DevStore> {
        let bucket_dir = root.join(EMBEDDED_BUCKET);
        tokio::fs::create_dir_all(&bucket_dir)
            .await
            .with_context(|| format!("creating embedded S3 bucket dir {}", bucket_dir.display()))?;

        let service = build_service(&root)?;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .context("binding embedded S3 listener")?;
        let addr = listener.local_addr().context("embedded S3 local_addr")?;

        tokio::spawn(serve_loop(listener, service));

        Ok(DevStore {
            endpoint: format!("http://{addr}"),
            bucket: EMBEDDED_BUCKET.to_string(),
            access_key_id: EMBEDDED_ACCESS_KEY.to_string(),
            secret_access_key: EMBEDDED_SECRET_KEY.to_string(),
            provenance: StoreProvenance::Embedded,
        })
    }

    /// Conventional env vars mesofact-dev injects so a workload's `[sources.r2]`
    /// can resolve in dev: `R2_ENDPOINT`, `R2_BUCKET`, `R2_ACCESS_KEY_ID`,
    /// `R2_SECRET_ACCESS_KEY`.
    pub fn env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("R2_ENDPOINT".to_string(), self.endpoint.clone()),
            ("R2_BUCKET".to_string(), self.bucket.clone()),
            ("R2_ACCESS_KEY_ID".to_string(), self.access_key_id.clone()),
            (
                "R2_SECRET_ACCESS_KEY".to_string(),
                self.secret_access_key.clone(),
            ),
        ]
    }
}

/// Permissive access layer: allow every request, authenticated or not. s3s only
/// consults `S3Access` when an auth provider is configured, so pairing this with
/// [`SimpleAuth`](s3s::auth::SimpleAuth) means signed `dev/dev` requests verify
/// AND unsigned loopback requests still pass — preserving the anonymous
/// dev-appliance contract while unblocking SigV4 clients.
struct AllowAllAccess;

#[async_trait::async_trait]
impl s3s::access::S3Access for AllowAllAccess {
    async fn check(&self, _cx: &mut s3s::access::S3AccessContext<'_>) -> s3s::S3Result<()> {
        Ok(())
    }
}

fn build_service(root: &Path) -> Result<s3s::service::S3Service> {
    use s3s::auth::SimpleAuth;
    use s3s::service::S3ServiceBuilder;
    // s3s_fs::Error doesn't impl std::error::Error, so map it by Display.
    let fs = s3s_fs::FileSystem::new(root)
        .map_err(|e| anyhow::anyhow!("opening s3s-fs at {}: {e:?}", root.display()))?;
    let mut builder = S3ServiceBuilder::new(fs);
    // Accept SigV4-signed requests. Without ANY auth provider s3s answers 501
    // ("no authentication provider") to every *signed* request — which breaks
    // the `S3Store`-based pointer/content reads the local publish→view loop
    // needs (W270 §9), and the R2Adapter signs too. SimpleAuth verifies the
    // dev/dev signature; AllowAllAccess keeps anonymous loopback access working,
    // so the surface stays the single-tenant dev appliance it was.
    builder.set_auth(SimpleAuth::from_single(
        EMBEDDED_ACCESS_KEY,
        EMBEDDED_SECRET_KEY,
    ));
    builder.set_access(AllowAllAccess);
    Ok(builder.build())
}

async fn serve_loop(listener: TcpListener, service: s3s::service::S3Service) {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder as ConnBuilder;

    let http = ConnBuilder::new(TokioExecutor::new());
    loop {
        let socket = match listener.accept().await {
            Ok((socket, _)) => socket,
            Err(e) => {
                tracing::warn!(error = %e, "embedded S3: accept failed");
                continue;
            }
        };
        // `.into_owned()` detaches the connection future from the borrowed
        // builder so it can be spawned with a `'static` lifetime.
        let conn = http
            .serve_connection(TokioIo::new(socket), service.clone())
            .into_owned();
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "embedded S3: connection ended");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    fn clear_env() {
        for var in REQUIRED_ENV {
            std::env::remove_var(var);
        }
    }

    #[tokio::test]
    async fn resolve_prefers_camp_injected_coordinates() {
        let _guard = ENV_LOCK.lock().await;
        clear_env();
        std::env::set_var("S3_ENDPOINT", "http://127.0.0.1:54321");
        std::env::set_var("S3_BUCKET", "dev");
        std::env::set_var("S3_ACCESS_KEY_ID", "ak");
        std::env::set_var("S3_SECRET_ACCESS_KEY", "sk");

        let tmp = tempfile::tempdir().unwrap();
        let coords = DevStore::resolve(tmp.path()).await.unwrap();
        assert_eq!(coords.provenance, StoreProvenance::Camp);
        assert_eq!(coords.endpoint, "http://127.0.0.1:54321");
        assert_eq!(coords.bucket, "dev");
        assert_eq!(
            coords.env_vars(),
            vec![
                ("R2_ENDPOINT".to_string(), "http://127.0.0.1:54321".to_string()),
                ("R2_BUCKET".to_string(), "dev".to_string()),
                ("R2_ACCESS_KEY_ID".to_string(), "ak".to_string()),
                ("R2_SECRET_ACCESS_KEY".to_string(), "sk".to_string()),
            ]
        );
        // The camp arm must not mint a bucket dir: the camp's store owns its
        // own data root, and a stray `.mesofact-dev/s3/` here would be an
        // embedded store nobody reads.
        assert!(!tmp.path().join("s3").exists());

        clear_env();
    }

    /// The case the embedded arm exists for: no camp, and `mes` still gets a
    /// store.
    #[tokio::test]
    async fn resolve_starts_an_embedded_store_when_no_camp_injected_anything() {
        let _guard = ENV_LOCK.lock().await;
        clear_env();

        let tmp = tempfile::tempdir().unwrap();
        let store = DevStore::resolve(tmp.path()).await.unwrap();
        assert_eq!(store.provenance, StoreProvenance::Embedded);
        assert_eq!(store.bucket, EMBEDDED_BUCKET);
        assert!(
            store.endpoint.starts_with("http://127.0.0.1:"),
            "{}",
            store.endpoint
        );
        assert!(tmp.path().join("s3").join(EMBEDDED_BUCKET).is_dir());

        // Bound, not merely formatted: the listener is accepting before
        // `resolve` returns, so a consumer that dials immediately connects.
        let addr = store.endpoint.trim_start_matches("http://");
        tokio::net::TcpStream::connect(addr)
            .await
            .expect("embedded S3 listener accepts");
    }

    /// A half-set injection is a miswired camp, and starting a second empty
    /// store would hide it behind a dev loop that looks fine until a published
    /// object turns up missing.
    #[tokio::test]
    async fn resolve_refuses_a_half_set_injection_instead_of_falling_back() {
        let _guard = ENV_LOCK.lock().await;
        clear_env();
        std::env::set_var("S3_ENDPOINT", "http://127.0.0.1:54321");
        // S3_BUCKET / S3_ACCESS_KEY_ID / S3_SECRET_ACCESS_KEY left unset.

        let tmp = tempfile::tempdir().unwrap();
        let err = DevStore::resolve(tmp.path()).await.unwrap_err().to_string();
        assert!(err.contains("S3_BUCKET"), "{err}");
        assert!(err.contains("S3_ACCESS_KEY_ID"), "{err}");
        assert!(err.contains("S3_SECRET_ACCESS_KEY"), "{err}");
        assert!(err.contains("S3_ENDPOINT"), "names what IS set too: {err}");
        // No store was started behind the error.
        assert!(!tmp.path().join("s3").exists());

        clear_env();
    }
}
