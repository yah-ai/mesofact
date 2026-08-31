//! Long-lived dev-tier SSR runtime (W174 pillar 4 / R449-F2).
//!
//! One V8 isolate hosts every `mode:"ssr"` route's Fetch handler. Routes are
//! pre-loaded at construction (cold-import cost paid once), then `dispatch`
//! re-uses the loaded handlers for each request. Replaces the `bun run`
//! subprocess + reverse-proxy hop that R434-F3 originally shipped.
//!
//! `JsRuntime` is `!Send`, so [`SsrRuntime`] owns a dedicated thread with a
//! current-thread tokio runtime; callers talk to it through a small Send
//! handle. Jobs flow over an mpsc channel — `register` blocks the caller
//! during cold-import; `dispatch` blocks the caller until the handler's
//! Response is fully realised.
//!
//! One `SsrRuntime`'s job loop (`while let Ok(job) = rx.recv()`, handler
//! fully awaited inside the loop body) serves one request at a time — no
//! second job is even *received* until the first settles. That is the
//! mpsc job-loop's shape, not a V8 constraint: V8 being single-threaded
//! prevents one isolate from running JS on two OS threads, it does not
//! prevent interleaved async on that one thread (Node is the
//! counterexample). W311 §1 measured the cost: 4 concurrent 100ms
//! dispatches on one isolate took 409ms, not the ~100ms Node-style
//! interleaving would give. [`SsrPool`] is the fix that was actually
//! adopted — N independent isolates on N threads, each still serial
//! internally, round-robined across so N requests get real parallelism.
//! An interleaving rewrite of the single job loop was considered and
//! deferred (W311 §1, move C) as the riskier restructure for a smaller win.

use anyhow::{anyhow, Context, Result};
use deno_core::error::ModuleLoaderError;
use deno_core::{
    resolve_import, JsRuntime, ModuleLoadResponse, ModuleLoader, ModuleSource, ModuleSourceCode,
    ModuleSpecifier, ModuleType, PollEventLoopOptions, RuntimeOptions,
};
use deno_permissions::PermissionsContainer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Once};
use std::thread::JoinHandle;

const SSR_BOOTSTRAP: &str = include_str!("../js/ssr_bootstrap.js");
const SSR_HARNESS: &str = include_str!("../js/ssr_harness.js");
/// Pure helpers shared with the SSG tier; re-exported by the SSR shim below.
const RUNTIME_PURE: &str = include_str!("../js/runtime_shim.js");
const SSR_RUNTIME_SHIM: &str = include_str!("../js/ssr_runtime_shim.js");
const HARNESS_SPECIFIER: &str = "mesofact-ssr:harness";
const RUNTIME_SPECIFIER: &str = "mesofact-ssr:runtime";
const RUNTIME_PURE_SPECIFIER: &str = "mesofact:runtime-pure";

/// Plain request shape handed in by the dev server.
#[derive(Debug, Clone, Serialize)]
pub struct DispatchRequest {
    pub method: String,
    pub url: String,
    /// (name, value) pairs. Values are already string-decoded; binary headers
    /// are not part of the Fetch contract.
    pub headers: Vec<(String, String)>,
    /// Request body bytes. `None` for GET/HEAD; empty Vec is allowed but the
    /// harness drops it before constructing the Request to match Fetch
    /// semantics.
    pub body: Option<Vec<u8>>,
}

/// Resolved R2 source coordinates handed to the isolate at boot (R444).
/// `mesofact::ssr::spawn` resolves `[sources.<name>]` (`kind = "r2"`) entries
/// from a workload's `mesofact.config.toml` against whatever env map the
/// caller passed in (dev: `DevS3::env_vars()`; prod: the receiver's real
/// process env) *before* this crate ever sees them — the isolate itself never
/// touches the config file or arbitrary env, only these already-resolved
/// values. Mirrors `packages/mesofact-runtime/src/adapters/r2.ts`'s `R2Config`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct R2SourceCoords {
    pub name: String,
    pub bucket: String,
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// Plain response shape returned by the harness; mirrors what the bun
/// wrapper used to forward via HTTP.
#[derive(Debug, Clone, Deserialize)]
pub struct DispatchResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Response body bytes. Empty Vec for status codes that mustn't carry a
    /// body — same as the upstream Response.
    #[serde(default, with = "serde_bytes_vec")]
    pub body: Vec<u8>,
}

mod serde_bytes_vec {
    use serde::de::{Deserializer, SeqAccess, Visitor};
    use std::fmt;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("byte sequence or Uint8Array")
            }
            fn visit_bytes<E>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }
            fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(v)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(out)
            }
        }
        d.deserialize_any(V)
    }
}

enum Job {
    Register {
        bundle: PathBuf,
        reply: mpsc::Sender<Result<()>>,
    },
    Dispatch {
        bundle: PathBuf,
        req: DispatchRequest,
        reply: mpsc::Sender<Result<DispatchResponse>>,
    },
    /// Mode 2 endpoint-callback verb (R756-F3 / W311 §2) — plain JSON in,
    /// plain JSON out, no `Request`/`Response` envelope.
    Invoke {
        bundle: PathBuf,
        hook: String,
        input: Value,
        reply: mpsc::Sender<Result<Value>>,
    },
    RegisterR2 {
        sources: Vec<R2SourceCoords>,
        reply: mpsc::Sender<Result<()>>,
    },
    Shutdown,
}

/// Handle to the dev-tier SSR isolate thread. Cheap to clone via `Arc` at
/// the caller; dropping the last handle does not stop the thread (callers
/// must call [`SsrRuntime::shutdown`] when they want it gone).
pub struct SsrRuntime {
    tx: mpsc::Sender<Job>,
    thread: Option<JoinHandle<()>>,
}

impl SsrRuntime {
    /// Boot a fresh isolate. Blocks until the bootstrap + harness have
    /// evaluated (so a configuration error in the extensions surfaces here
    /// rather than on the first request).
    ///
    /// `env` is `Object.assign`-ed onto `globalThis.process.env` right after
    /// the bootstrap script runs (R444) — the in-process V8 isolate can't
    /// inherit the host process's real env the way a bun subprocess did, so
    /// the caller (`mesofact::ssr::spawn`) hands in whatever env map route
    /// code and `[sources.r2]` resolution should see.
    pub fn start(env: Vec<(String, String)>) -> Result<Self> {
        ensure_crypto_provider();
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let thread = std::thread::Builder::new()
            .name("mesofact-ssr".into())
            .spawn(move || run_isolate(rx, ready_tx, env))
            .context("spawning SSR isolate thread")?;
        ready_rx
            .recv()
            .context("SSR isolate thread died during startup")??;
        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    fn call<R>(&self, job: impl FnOnce(mpsc::Sender<Result<R>>) -> Job) -> Result<R> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(job(reply_tx))
            .map_err(|_| anyhow!("SSR isolate thread is gone"))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("SSR isolate thread dropped the reply"))?
    }

    /// Pre-load a route's render_entrypoint module. Idempotent (re-loading
    /// replaces the prior handler). `bundle` must be an absolute file path.
    pub fn register(&self, bundle: &Path) -> Result<()> {
        self.call(|reply| Job::Register {
            bundle: bundle.to_path_buf(),
            reply,
        })
    }

    /// Invoke a previously-registered route's Fetch handler with `req` and
    /// return its Response. Blocks the calling thread until the handler
    /// settles; safe to call from a tokio task (it uses a sync mpsc reply,
    /// no nested runtime).
    pub fn dispatch(&self, bundle: &Path, req: DispatchRequest) -> Result<DispatchResponse> {
        self.call(|reply| Job::Dispatch {
            bundle: bundle.to_path_buf(),
            req,
            reply,
        })
    }

    /// Invoke a hook exported by a previously-registered route's module —
    /// the Mode 2 (endpoint-callback) call surface (R756-F3 / W311 §2).
    /// Unlike [`SsrRuntime::dispatch`], no `Request`/`Response` envelope
    /// crosses the Rust↔V8 boundary: `input` and the returned verdict are
    /// both plain JSON, so a small yes/no hook (e.g. `/readyz`'s `app`
    /// check) pays only for what it actually needs to move — no header vec,
    /// no byte body.
    pub fn invoke(&self, bundle: &Path, hook: &str, input: Value) -> Result<Value> {
        self.call(|reply| Job::Invoke {
            bundle: bundle.to_path_buf(),
            hook: hook.to_string(),
            input,
            reply,
        })
    }

    /// Register resolved R2 source coordinates so `r2(name)` inside the
    /// isolate's `@mesofact/runtime` shim resolves against them (R444).
    /// Idempotent — re-registering a name replaces its adapter.
    pub fn register_r2_sources(&self, sources: &[R2SourceCoords]) -> Result<()> {
        self.call(|reply| Job::RegisterR2 {
            sources: sources.to_vec(),
            reply,
        })
    }

    /// Explicit shutdown — sends a stop job and joins the isolate thread.
    /// Drop falls back to this if it wasn't called explicitly.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Job::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for SsrRuntime {
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Default isolate count when a caller doesn't pick one explicitly (R756-F2).
///
/// Measured 2026-08-13 on this dev box: booting an empty isolate (no routes
/// registered) costs ~27MB RSS for the first one (one-time V8 platform init,
/// paid regardless of pool size) and ~6-9MB RSS steady-state for each
/// additional one. `num_cpus` is the wrong default per W311 — "sizing is not
/// free on a cheap box" — and register() cost (paid per isolate, per route,
/// at boot and every gen flip) dominates isolate-count cost long before RSS
/// does. 4 is a deliberately small, non-`num_cpus` default; override via
/// [`SsrPool::start`]'s `size` argument for boxes that can afford more.
pub const DEFAULT_POOL_SIZE: usize = 4;

/// N independent [`SsrRuntime`] isolates, each on its own dedicated thread,
/// round-robined across (R756-F2 — W311 §1's adopted "move B"). Real
/// parallelism, not a lock dance: V8 isolates are independent heaps, so N
/// isolates genuinely run N requests' JS concurrently. Each isolate still
/// serves its own jobs one at a time internally (unchanged from
/// [`SsrRuntime`]'s job loop) — the pool is what buys inter-request overlap.
///
/// **Contract change this makes load-bearing**: consecutive requests may
/// land on *different* isolates, so route handlers may not rely on
/// module-level JS state surviving across requests. The bootstrap already
/// half-declared this before the pool existed — `setInterval` is
/// deliberately unbound because "one isolate serves every request, so a
/// repeating timer leaks across them" (`bootstrap_binds_base64_and_request_scoped_timers`).
/// A pool of >1 makes that the general case, not an edge case.
///
/// `register`/`register_r2_sources` fan out to every isolate in the pool
/// (each one needs its own module-cache entry — that per-isolate cost, not
/// isolate RSS, is the real budget line per W311 §1) and fail fast on the
/// first isolate that errors, mirroring `SsrRuntime`'s own fail-fast
/// contract.
pub struct SsrPool {
    isolates: Vec<SsrRuntime>,
    next: AtomicUsize,
}

impl SsrPool {
    /// Boot `size` isolates (each gets its own copy of `env`; clamped to at
    /// least 1). Blocks until every isolate's bootstrap has evaluated —
    /// same fail-fast-at-boot contract as [`SsrRuntime::start`].
    pub fn start(env: Vec<(String, String)>, size: usize) -> Result<Self> {
        let size = size.max(1);
        let mut isolates = Vec::with_capacity(size);
        for _ in 0..size {
            isolates.push(SsrRuntime::start(env.clone())?);
        }
        Ok(Self {
            isolates,
            next: AtomicUsize::new(0),
        })
    }

    /// Number of isolates in the pool.
    pub fn size(&self) -> usize {
        self.isolates.len()
    }

    /// Pre-load `bundle` on every isolate in the pool — a request dispatched
    /// to any of them must find the handler already registered.
    pub fn register(&self, bundle: &Path) -> Result<()> {
        for rt in &self.isolates {
            rt.register(bundle)?;
        }
        Ok(())
    }

    /// Register resolved R2 source coordinates on every isolate in the pool.
    pub fn register_r2_sources(&self, sources: &[R2SourceCoords]) -> Result<()> {
        for rt in &self.isolates {
            rt.register_r2_sources(sources)?;
        }
        Ok(())
    }

    /// Dispatch to the next isolate in round-robin order. Blocks the calling
    /// thread until that isolate's handler settles — same contract as
    /// [`SsrRuntime::dispatch`], safe to call from a `spawn_blocking` task.
    pub fn dispatch(&self, bundle: &Path, req: DispatchRequest) -> Result<DispatchResponse> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.isolates.len();
        self.isolates[idx].dispatch(bundle, req)
    }

    /// Invoke a hook on the next isolate in round-robin order. See
    /// [`SsrRuntime::invoke`].
    pub fn invoke(&self, bundle: &Path, hook: &str, input: Value) -> Result<Value> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.isolates.len();
        self.isolates[idx].invoke(bundle, hook, input)
    }
}

static CRYPTO_PROVIDER_INIT: Once = Once::new();

/// Install the process-wide rustls `CryptoProvider` exactly once. deno_fetch's
/// HTTP client always builds a TLS-capable connector — even for a plain
/// `http://` request — and both `aws-lc-rs` and `ring` end up in this crate's
/// dependency graph (different `deno_*` crates pick different rustls
/// features), so rustls can't auto-select a default: the first real
/// `fetch()` call panics without this. Idempotent — `install_default`'s
/// "already installed" error (e.g. a peer crate installed one first) is
/// expected and ignored, not a startup failure.
fn ensure_crypto_provider() {
    CRYPTO_PROVIDER_INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Module loader: file-system ESM plus the embedded harness module.
struct SsrModuleLoader;

impl ModuleLoader for SsrModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: deno_core::ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        // SSR bundles keep `@mesofact/runtime` external (bundle.rs `ssr_bundle`),
        // same as SSG bundles, so the bare specifier lands here at register time.
        if specifier == "@mesofact/runtime" || specifier == RUNTIME_SPECIFIER {
            return ModuleSpecifier::parse(RUNTIME_SPECIFIER)
                .map_err(|e| ModuleLoaderError::generic(e.to_string()));
        }
        if specifier == RUNTIME_PURE_SPECIFIER {
            return ModuleSpecifier::parse(RUNTIME_PURE_SPECIFIER)
                .map_err(|e| ModuleLoaderError::generic(e.to_string()));
        }
        if specifier == HARNESS_SPECIFIER {
            return ModuleSpecifier::parse(HARNESS_SPECIFIER)
                .map_err(|e| ModuleLoaderError::generic(e.to_string()));
        }
        resolve_import(specifier, referrer).map_err(ModuleLoaderError::from_err)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&deno_core::ModuleLoadReferrer>,
        _options: deno_core::ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let spec = module_specifier.clone();
        let load = || -> Result<ModuleSource, ModuleLoaderError> {
            let code: String = match spec.as_str() {
                HARNESS_SPECIFIER => SSR_HARNESS.to_string(),
                RUNTIME_SPECIFIER => SSR_RUNTIME_SHIM.to_string(),
                RUNTIME_PURE_SPECIFIER => RUNTIME_PURE.to_string(),
                _ => {
                    let path = spec.to_file_path().map_err(|()| {
                        ModuleLoaderError::generic(format!(
                            "only file:// modules can be loaded in the SSR isolate (got {spec})"
                        ))
                    })?;
                    std::fs::read_to_string(&path).map_err(|e| {
                        ModuleLoaderError::generic(format!(
                            "failed reading module {}: {e}",
                            path.display()
                        ))
                    })?
                }
            };
            Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(code.into()),
                &spec,
                None,
            ))
        };
        ModuleLoadResponse::Sync(load())
    }
}

fn build_permissions() -> PermissionsContainer {
    let parser = deno_permissions::RuntimePermissionDescriptorParser::new(
        sys_traits::impls::RealSys,
    );
    PermissionsContainer::allow_all(Arc::new(parser))
}

fn build_runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(SsrModuleLoader)),
        extensions: vec![
            deno_webidl::deno_webidl::init(),
            deno_web::deno_web::init(
                Arc::new(deno_web::BlobStore::default()),
                None,
                false,
                deno_web::InMemoryBroadcastChannel::default(),
            ),
            deno_net::deno_net::init(None, None),
            deno_fetch::deno_fetch::init(deno_fetch::Options::default()),
        ],
        ..Default::default()
    })
}

fn run_isolate(rx: mpsc::Receiver<Job>, ready: mpsc::Sender<Result<()>>, env: Vec<(String, String)>) {
    let tokio_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("building SSR tokio runtime: {e}")));
            return;
        }
    };
    tokio_rt.block_on(async move {
        let mut runtime = build_runtime();

        // Make the permissions container available to deno_fetch/deno_web ops
        // (they read it out of the op state).
        runtime
            .op_state()
            .borrow_mut()
            .put(build_permissions());

        let init = async {
            runtime
                .execute_script("mesofact-ssr:bootstrap", SSR_BOOTSTRAP)
                .map_err(|e| anyhow!("SSR bootstrap failed: {e}"))?;
            // R444: fold the caller's env onto the empty `process.env` the
            // bootstrap just installed, before the harness (or any route
            // module) loads — so a top-level read at module-eval time still
            // sees it, not just a read from inside a request handler.
            if !env.is_empty() {
                let mut env_obj = serde_json::Map::with_capacity(env.len());
                for (k, v) in &env {
                    env_obj.insert(k.clone(), Value::String(v.clone()));
                }
                let script = format!(
                    "Object.assign(globalThis.process.env, {});",
                    serde_json::to_string(&env_obj)?
                );
                runtime
                    .execute_script("mesofact-ssr:env", script)
                    .map_err(|e| anyhow!("injecting env into SSR isolate: {e}"))?;
            }
            let harness_spec = ModuleSpecifier::parse(HARNESS_SPECIFIER).unwrap();
            let id = runtime
                .load_side_es_module(&harness_spec)
                .await
                .map_err(|e| anyhow!("loading SSR harness: {e}"))?;
            let receiver = runtime.mod_evaluate(id);
            runtime
                .run_event_loop(PollEventLoopOptions::default())
                .await
                .map_err(|e| anyhow!("evaluating SSR harness: {e}"))?;
            receiver.await.map_err(|e| anyhow!("SSR harness threw: {e}"))?;
            Ok::<(), anyhow::Error>(())
        };
        if let Err(e) = init.await {
            let _ = ready.send(Err(e));
            return;
        }
        let _ = ready.send(Ok(()));

        while let Ok(job) = rx.recv() {
            match job {
                Job::Shutdown => break,
                Job::Register { bundle, reply } => {
                    let r = call_harness(&mut runtime, "register", &bundle, None).await.map(|_| ());
                    let _ = reply.send(r);
                }
                Job::Dispatch { bundle, req, reply } => {
                    let input = match serde_json::to_value(&req) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = reply.send(Err(anyhow!("serialising request: {e}")));
                            continue;
                        }
                    };
                    let r = dispatch_harness(&mut runtime, &bundle, input).await;
                    let _ = reply.send(r);
                }
                Job::Invoke { bundle, hook, input, reply } => {
                    let r = invoke_harness(&mut runtime, &bundle, &hook, input).await;
                    let _ = reply.send(r);
                }
                Job::RegisterR2 { sources, reply } => {
                    let input = match serde_json::to_value(&sources) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = reply.send(Err(anyhow!("serialising r2 sources: {e}")));
                            continue;
                        }
                    };
                    let r = call_bridge(&mut runtime, "registerR2Sources", input)
                        .await
                        .map(|_| ());
                    let _ = reply.send(r);
                }
            }
        }
    });
}

/// Like `call_harness` for `dispatch`, but pulls the result straight out of
/// V8 into a `DispatchResponse` so the Uint8Array body survives — going via
/// `serde_json::Value` would fail on the byte-array path.
async fn dispatch_harness(
    runtime: &mut JsRuntime,
    bundle: &Path,
    input: Value,
) -> Result<DispatchResponse> {
    let url = ModuleSpecifier::from_file_path(bundle)
        .map_err(|()| anyhow!("bundle path is not absolute: {}", bundle.display()))?;
    let url_json = serde_json::to_string(url.as_str())?;
    let script = format!(
        "globalThis.__mesofact_ssr.dispatch({url_json}, {})",
        serde_json::to_string(&input)?
    );
    let promise = runtime
        .execute_script("mesofact-ssr:call", script)
        .map_err(|e| anyhow!("dispatch dispatch failed: {e}"))?;
    let watcher = runtime.resolve(promise);
    let global = runtime
        .with_event_loop_promise(watcher, PollEventLoopOptions::default())
        .await
        .map_err(|e| anyhow!("dispatch failed: {e}"))?;
    deno_core::scope!(scope, runtime);
    let local = deno_core::v8::Local::new(scope, global);
    deno_core::serde_v8::from_v8::<DispatchResponse>(scope, local)
        .map_err(|e| anyhow!("deserialising SSR response: {e}"))
}

async fn call_harness(
    runtime: &mut JsRuntime,
    method: &str,
    bundle: &Path,
    input: Option<Value>,
) -> Result<Value> {
    let url = ModuleSpecifier::from_file_path(bundle)
        .map_err(|()| anyhow!("bundle path is not absolute: {}", bundle.display()))?;
    let url_json = serde_json::to_string(url.as_str())?;
    let script = match input {
        Some(v) => format!(
            "globalThis.__mesofact_ssr.{method}({url_json}, {})",
            serde_json::to_string(&v)?
        ),
        None => format!("globalThis.__mesofact_ssr.{method}({url_json})"),
    };
    let promise = runtime
        .execute_script("mesofact-ssr:call", script)
        .map_err(|e| anyhow!("{method} dispatch failed: {e}"))?;
    let watcher = runtime.resolve(promise);
    let global = runtime
        .with_event_loop_promise(watcher, PollEventLoopOptions::default())
        .await
        .map_err(|e| anyhow!("{method} failed: {e}"))?;
    deno_core::scope!(scope, runtime);
    let local = deno_core::v8::Local::new(scope, global);
    let value: Value = deno_core::serde_v8::from_v8(scope, local)
        .map_err(|e| anyhow!("{method} returned an unserializable value: {e}"))?;
    Ok(value)
}

/// Call `globalThis.__mesofact_ssr.invoke(url, hook, input)` — the Mode 2
/// verb (R756-F3 / W311 §2). Structurally the same shape as [`call_harness`]
/// (plain JSON in, plain JSON out via `serde_json::Value` — no `Uint8Array`
/// body to dodge, unlike [`dispatch_harness`]) but takes the extra `hook`
/// argument `call_harness`'s single-`method`-selects-the-op shape has no
/// room for.
async fn invoke_harness(
    runtime: &mut JsRuntime,
    bundle: &Path,
    hook: &str,
    input: Value,
) -> Result<Value> {
    let url = ModuleSpecifier::from_file_path(bundle)
        .map_err(|()| anyhow!("bundle path is not absolute: {}", bundle.display()))?;
    let url_json = serde_json::to_string(url.as_str())?;
    let hook_json = serde_json::to_string(hook)?;
    let script = format!(
        "globalThis.__mesofact_ssr.invoke({url_json}, {hook_json}, {})",
        serde_json::to_string(&input)?
    );
    let promise = runtime
        .execute_script("mesofact-ssr:call", script)
        .map_err(|e| anyhow!("invoke dispatch failed: {e}"))?;
    let watcher = runtime.resolve(promise);
    let global = runtime
        .with_event_loop_promise(watcher, PollEventLoopOptions::default())
        .await
        .map_err(|e| anyhow!("invoke failed: {e}"))?;
    deno_core::scope!(scope, runtime);
    let local = deno_core::v8::Local::new(scope, global);
    deno_core::serde_v8::from_v8(scope, local)
        .map_err(|e| anyhow!("invoke returned an unserializable value: {e}"))
}

/// Call a no-bundle bridge method on `globalThis.__mesofact_ssr` (R444's
/// `registerR2Sources`, as opposed to `call_harness`'s per-bundle
/// register/dispatch which key on a registered module's URL).
async fn call_bridge(runtime: &mut JsRuntime, method: &str, input: Value) -> Result<Value> {
    let script = format!(
        "globalThis.__mesofact_ssr.{method}({})",
        serde_json::to_string(&input)?
    );
    let promise = runtime
        .execute_script("mesofact-ssr:call", script)
        .map_err(|e| anyhow!("{method} dispatch failed: {e}"))?;
    let watcher = runtime.resolve(promise);
    let global = runtime
        .with_event_loop_promise(watcher, PollEventLoopOptions::default())
        .await
        .map_err(|e| anyhow!("{method} failed: {e}"))?;
    deno_core::scope!(scope, runtime);
    let local = deno_core::v8::Local::new(scope, global);
    let value: Value = deno_core::serde_v8::from_v8(scope, local)
        .map_err(|e| anyhow!("{method} returned an unserializable value: {e}"))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end smoke: register a fixture route that returns a static
    /// Response, dispatch a GET to it, expect status + body to round-trip.
    /// Exercises the bootstrap → harness → handler chain in-process.
    #[test]
    fn dispatch_returns_handlers_response() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("ping.js");
        std::fs::write(
            &bundle,
            "export default async function (req) {\n\
                return new Response('pong from ' + req.method, {\n\
                  status: 200,\n\
                  headers: { 'content-type': 'text/plain' },\n\
                });\n\
              }\n",
        )
        .unwrap();

        let rt = SsrRuntime::start(Vec::new()).expect("ssr runtime starts");
        rt.register(&bundle).expect("register");
        let resp = rt
            .dispatch(
                &bundle,
                DispatchRequest {
                    method: "GET".into(),
                    url: "http://dev/api/ping".into(),
                    headers: vec![],
                    body: None,
                },
            )
            .expect("dispatch");
        assert_eq!(resp.status, 200);
        assert_eq!(String::from_utf8(resp.body).unwrap(), "pong from GET");
        let ct = resp
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert!(ct.starts_with("text/plain"), "got content-type {ct}");
    }

    /// R756-F3: `invoke` reuses the registered route's default Fetch handler
    /// (readyz's contract stays `Request -> Response`, per W311 §2's
    /// "vanilla is a plain Fetch handler" precedent) but the boundary
    /// crossing is plain JSON both ways — no header vec, no byte body, and
    /// the handler's response body is never even read.
    #[test]
    fn invoke_calls_the_readyz_hook_and_returns_a_json_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("readyz.js");
        std::fs::write(
            &bundle,
            "export default async function (req) {\n\
               const fail = new URL(req.url).searchParams.has('fail');\n\
               return new Response(fail ? 'not ready' : 'ok', {\n\
                 status: fail ? 503 : 200,\n\
               });\n\
             }\n",
        )
        .unwrap();

        let rt = SsrRuntime::start(Vec::new()).expect("ssr runtime starts");
        rt.register(&bundle).expect("register");

        let ok = rt
            .invoke(
                &bundle,
                "readyz",
                serde_json::json!({"method": "GET", "url": "http://dev/readyz"}),
            )
            .expect("invoke");
        assert_eq!(ok, serde_json::json!({"status": 200}));

        let failing = rt
            .invoke(
                &bundle,
                "readyz",
                serde_json::json!({"method": "GET", "url": "http://dev/readyz?fail"}),
            )
            .expect("invoke");
        assert_eq!(failing, serde_json::json!({"status": 503}));
    }

    /// An unrecognised hook name fails loud rather than silently answering
    /// ready — the additive-only asymmetry (`an_app_readyz_cannot_overrule_
    /// the_engine`) depends on a broken hook reporting as a failure, not a
    /// pass.
    #[test]
    fn invoke_rejects_an_unknown_hook_name() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("readyz.js");
        std::fs::write(
            &bundle,
            "export default async function () {\n\
               return new Response('ok');\n\
             }\n",
        )
        .unwrap();

        let rt = SsrRuntime::start(Vec::new()).expect("ssr runtime starts");
        rt.register(&bundle).expect("register");

        let err = rt
            .invoke(&bundle, "bogus", serde_json::json!({}))
            .unwrap_err();
        assert!(
            err.to_string().contains("bogus"),
            "error should name the unknown hook, got {err}"
        );
    }

    /// R746-S4: `atob`/`btoa` and `setTimeout`/`clearTimeout` are bound in the
    /// SSR bootstrap. The base64 module was already being loaded and simply
    /// never assigned to `globalThis`, and timers were never loaded at all, so
    /// route code calling either got a bare `ReferenceError` on capability that
    /// was already compiled in. `setInterval` stays unbound on purpose — one
    /// isolate serves every request, so a repeating timer leaks across them.
    #[test]
    fn bootstrap_binds_base64_and_request_scoped_timers() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("globals.js");
        std::fs::write(
            &bundle,
            "export default async function () {\n\
               const delayed = await new Promise((res) => setTimeout(() => res('tick'), 1));\n\
               const cancelled = clearTimeout(setTimeout(() => {}, 50)) === undefined;\n\
               return new Response(JSON.stringify({\n\
                 b64: btoa('mesofact'),\n\
                 round: atob(btoa('mesofact')),\n\
                 delayed,\n\
                 cancelled,\n\
                 interval: typeof globalThis.setInterval,\n\
               }), { status: 200 });\n\
             }\n",
        )
        .unwrap();

        let rt = SsrRuntime::start(Vec::new()).expect("ssr runtime starts");
        rt.register(&bundle).expect("register");
        let resp = rt
            .dispatch(
                &bundle,
                DispatchRequest {
                    method: "GET".into(),
                    url: "http://dev/".into(),
                    headers: vec![],
                    body: None,
                },
            )
            .expect("dispatch");
        assert_eq!(resp.status, 200);
        let body: Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(body["b64"], "bWVzb2ZhY3Q=");
        assert_eq!(body["round"], "mesofact");
        // The timer actually fired — the dispatch path drives the event loop.
        assert_eq!(body["delayed"], "tick");
        assert_eq!(body["cancelled"], true);
        // Deliberately absent: a repeating timer would outlive its request.
        assert_eq!(body["interval"], "undefined");
    }

    /// R637: SSR bundles keep `@mesofact/runtime` external, so a `mode:"ssr"`
    /// entrypoint that imports from the barrel must still register. Before the
    /// SSR loader answered that specifier, `register` threw
    /// `Relative import path "@mesofact/runtime" not prefixed with …` and
    /// `mesofact::ssr::spawn` failed the whole SSR child at startup — which is
    /// exactly what analytics.yah.dev's `import { r2 } from "@mesofact/runtime"`
    /// hit. Pins both halves: the pure helpers work, and a source nobody
    /// registered (no `register_r2_sources` call in this test) rejects with
    /// the explanatory not-registered error rather than a TypeError about a
    /// missing method.
    #[test]
    fn registers_bundle_importing_the_runtime_barrel() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("with_runtime.js");
        std::fs::write(
            &bundle,
            "import { r2, weaveHead } from \"@mesofact/runtime\";\n\
             export default async function () {\n\
               let sourceErr = 'none';\n\
               try {\n\
                 await r2('analytics').fetch('pointer.json');\n\
               } catch (e) {\n\
                 sourceErr = e.name + ': ' + e.message;\n\
               }\n\
               const head = weaveHead('<html><head></head></html>', { title: 'ok' });\n\
               return new Response(JSON.stringify({ sourceErr, head }), {\n\
                 status: 200,\n\
                 headers: { 'content-type': 'application/json' },\n\
               });\n\
             }\n",
        )
        .unwrap();

        let rt = SsrRuntime::start(Vec::new()).expect("ssr runtime starts");
        rt.register(&bundle).expect("bundle importing @mesofact/runtime registers");
        let resp = rt
            .dispatch(
                &bundle,
                DispatchRequest {
                    method: "GET".into(),
                    url: "http://dev/".into(),
                    headers: vec![],
                    body: None,
                },
            )
            .expect("dispatch");
        assert_eq!(resp.status, 200);
        let body: Value = serde_json::from_slice(&resp.body).unwrap();
        let source_err = body["sourceErr"].as_str().unwrap();
        assert!(
            source_err.starts_with("SourceNotRegisteredError:"),
            "expected the explanatory source error, got {source_err}"
        );
        assert!(
            source_err.contains("mesofact.config.toml"),
            "the error should point at where to declare the source, got {source_err}"
        );
        // The pure half of the barrel is fully usable at request time.
        assert_eq!(
            body["head"].as_str().unwrap(),
            "<html><head><title>ok</title></head></html>"
        );
    }

    /// DISCOVERED AND FIXED IN R756-F6: `defineReadyz` — the documented,
    /// dogfooded way to write a `/readyz` handler (`examples/hello/src/
    /// readyz.ts` imports it) — was missing from the isolate's
    /// `@mesofact/runtime` shim entirely. A real app using it failed to
    /// *register*, which fails `mesofact::ssr::spawn` at boot, so the seam
    /// R756-F5 shipped was reachable only by handlers that hand-rolled the
    /// wire format. Nothing caught it because every existing fixture writes
    /// its Response inline.
    ///
    /// Also pins the wire format, which is the seam's whole point: an
    /// operator running `curl /readyz?verbose` must not be able to tell
    /// whether Rust or TSX answered.
    #[test]
    fn define_readyz_is_usable_inside_the_isolate() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("readyz.js");
        std::fs::write(
            &bundle,
            "import { defineReadyz } from \"@mesofact/runtime\";\n\
             export default defineReadyz([\n\
               { name: 'up', check: () => true },\n\
               { name: 'db', check: () => new URL('http://x/?' + globalThis.__q).searchParams.has('fail') === false },\n\
             ]);\n",
        )
        .unwrap();

        let rt = SsrRuntime::start(Vec::new()).expect("ssr runtime starts");
        rt.register(&bundle).expect("a bundle using defineReadyz registers");

        let ok = rt
            .invoke(
                &bundle,
                "readyz",
                serde_json::json!({"method": "GET", "url": "http://dev/readyz"}),
            )
            .expect("invoke");
        assert_eq!(ok, serde_json::json!({"status": 200}));

        // …and the verbose listing matches the kube-apiserver shape the Rust
        // side emits, check-for-check.
        let resp = rt
            .dispatch(
                &bundle,
                DispatchRequest {
                    method: "GET".into(),
                    url: "http://dev/readyz?verbose".into(),
                    headers: vec![],
                    body: None,
                },
            )
            .expect("dispatch");
        assert_eq!(resp.status, 200);
        assert_eq!(
            String::from_utf8(resp.body).unwrap(),
            "[+]up ok\n[+]db ok\nreadyz check passed\n"
        );
    }

    /// R756-F2: `register` must fan out to every isolate, not just one that
    /// happens to answer first — otherwise round-robin dispatch would 404 on
    /// whichever isolates never saw the bundle.
    #[test]
    fn pool_registers_on_every_isolate_so_round_robin_dispatch_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("ping.js");
        std::fs::write(
            &bundle,
            "export default async function () {\n\
               return new Response('pong');\n\
             }\n",
        )
        .unwrap();

        let pool = SsrPool::start(Vec::new(), 3).expect("pool boots");
        pool.register(&bundle).expect("registers on every isolate");

        // More dispatches than isolates so round-robin wraps at least once.
        for _ in 0..6 {
            let resp = pool
                .dispatch(
                    &bundle,
                    DispatchRequest {
                        method: "GET".into(),
                        url: "http://dev/".into(),
                        headers: vec![],
                        body: None,
                    },
                )
                .expect("dispatch");
            assert_eq!(resp.status, 200);
            assert_eq!(String::from_utf8(resp.body).unwrap(), "pong");
        }
    }

    /// R756-F2 / W311 §1 — pins the actual result the pool exists for:
    /// concurrent dispatches get real parallelism, not the ~409ms-for-4
    /// serialized behaviour a single isolate measured. Asserts an order of
    /// magnitude (parallel ~100ms vs serialized ~400ms), not a millisecond
    /// figure, so a loaded CI box doesn't flake it.
    #[test]
    fn pool_gives_real_parallelism_not_serialized_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("slow.js");
        std::fs::write(
            &bundle,
            "export default async function () {\n\
               await new Promise((res) => setTimeout(res, 100));\n\
               return new Response('done');\n\
             }\n",
        )
        .unwrap();

        let pool = Arc::new(SsrPool::start(Vec::new(), 4).expect("pool boots"));
        pool.register(&bundle).expect("register");

        let start = std::time::Instant::now();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let pool = Arc::clone(&pool);
                let bundle = bundle.clone();
                std::thread::spawn(move || {
                    pool.dispatch(
                        &bundle,
                        DispatchRequest {
                            method: "GET".into(),
                            url: "http://dev/".into(),
                            headers: vec![],
                            body: None,
                        },
                    )
                    .unwrap()
                })
            })
            .collect();
        for h in handles {
            let resp = h.join().unwrap();
            assert_eq!(resp.status, 200);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 300,
            "expected pool parallelism (~100ms), dispatch took {elapsed:?} \
             (a single serialized isolate measured ~409ms for this exact shape, W311 §1)",
        );
    }
}

