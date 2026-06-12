//! Build-time JS execution on deno_core (W174 pillar 4). One V8 isolate per
//! build hosts three jobs the Bun pipeline did in-process: evaluating the
//! bundled `mesofact.routes.ts`, probing SSR entrypoints' default export,
//! and rendering static/spa routes to HTML (the SSG pass, R449-F1).
//!
//! `JsRuntime` is `!Send`, so the executor owns a dedicated thread with a
//! current-thread tokio runtime; callers talk to it through a small
//! synchronous handle. Renders run sequentially — same ordering the TS
//! prerender driver has.

use anyhow::{anyhow, Context, Result};
use deno_core::error::ModuleLoaderError;
use deno_core::{
    resolve_import, JsRuntime, ModuleLoadResponse, ModuleLoader, ModuleSource, ModuleSourceCode,
    ModuleSpecifier, ModuleType, PollEventLoopOptions, RuntimeOptions,
};
use serde_json::Value;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

const BOOTSTRAP: &str = include_str!("../js/bootstrap.js");
const RUNTIME_SHIM: &str = include_str!("../js/runtime_shim.js");
const HARNESS: &str = include_str!("../js/harness.js");

const RUNTIME_SPECIFIER: &str = "mesofact:runtime";
const HARNESS_SPECIFIER: &str = "mesofact:harness";

/// Module loader: file-system ESM plus two virtual modules. Bare specifier
/// "@mesofact/runtime" resolves to the embedded shim — the same external
/// boundary the emitted `dist/server/*.js` artifacts keep (bundle.ts keeps
/// the runtime external so the publisher unit dedupes it).
struct SsgModuleLoader;

impl ModuleLoader for SsgModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: deno_core::ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        if specifier == "@mesofact/runtime" || specifier == RUNTIME_SPECIFIER {
            return ModuleSpecifier::parse(RUNTIME_SPECIFIER)
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
                RUNTIME_SPECIFIER => RUNTIME_SHIM.to_string(),
                HARNESS_SPECIFIER => HARNESS.to_string(),
                _ => {
                    let path = spec.to_file_path().map_err(|()| {
                        ModuleLoaderError::generic(format!(
                            "only file:// modules can be loaded during SSG (got {spec}); node/bun builtins are not available at build time"
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

enum Job {
    EvalRoutes {
        bundle: std::path::PathBuf,
        reply: mpsc::Sender<Result<Value>>,
    },
    ProbeDefault {
        bundle: std::path::PathBuf,
        reply: mpsc::Sender<Result<Value>>,
    },
    Render {
        bundle: std::path::PathBuf,
        input: Value,
        reply: mpsc::Sender<Result<Value>>,
    },
    Shutdown,
}

/// Handle to the SSG isolate thread. Dropping shuts the isolate down.
pub struct SsgRuntime {
    tx: mpsc::Sender<Job>,
    thread: Option<JoinHandle<()>>,
}

impl SsgRuntime {
    pub fn start() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let thread = std::thread::Builder::new()
            .name("mesofact-ssg".into())
            .spawn(move || run_isolate(rx, ready_tx))
            .context("spawning SSG isolate thread")?;
        ready_rx
            .recv()
            .context("SSG isolate thread died during startup")??;
        Ok(Self { tx, thread: Some(thread) })
    }

    fn call(&self, job: impl FnOnce(mpsc::Sender<Result<Value>>) -> Job) -> Result<Value> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(job(reply_tx))
            .map_err(|_| anyhow!("SSG isolate thread is gone"))?;
        reply_rx.recv().map_err(|_| anyhow!("SSG isolate thread dropped the reply"))?
    }

    /// Evaluate a bundled routes module → the RoutesConfig as JSON.
    pub fn eval_routes(&self, bundle: &Path) -> Result<Value> {
        self.call(|reply| Job::EvalRoutes { bundle: bundle.to_path_buf(), reply })
    }

    /// `typeof default` probe for an SSR bundle ({ kind: "function" | ... }).
    pub fn probe_default(&self, bundle: &Path) -> Result<Value> {
        self.call(|reply| Job::ProbeDefault { bundle: bundle.to_path_buf(), reply })
    }

    /// Render one emission; `input` is the harness's render() input object.
    /// Returns `{ html, tags }`.
    pub fn render(&self, bundle: &Path, input: Value) -> Result<Value> {
        self.call(|reply| Job::Render { bundle: bundle.to_path_buf(), input, reply })
    }
}

impl Drop for SsgRuntime {
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run_isolate(rx: mpsc::Receiver<Job>, ready: mpsc::Sender<Result<()>>) {
    let tokio_rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("building tokio runtime: {e}")));
            return;
        }
    };
    tokio_rt.block_on(async move {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(SsgModuleLoader)),
            ..Default::default()
        });

        let init = async {
            runtime
                .execute_script("mesofact:bootstrap", BOOTSTRAP)
                .map_err(|e| anyhow!("SSG bootstrap failed: {e}"))?;
            let harness_spec = ModuleSpecifier::parse(HARNESS_SPECIFIER).unwrap();
            let id = runtime
                .load_side_es_module(&harness_spec)
                .await
                .map_err(|e| anyhow!("loading SSG harness: {e}"))?;
            let receiver = runtime.mod_evaluate(id);
            runtime
                .run_event_loop(PollEventLoopOptions::default())
                .await
                .map_err(|e| anyhow!("evaluating SSG harness: {e}"))?;
            receiver.await.map_err(|e| anyhow!("SSG harness threw: {e}"))?;
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
                Job::EvalRoutes { bundle, reply } => {
                    let r = call_harness(&mut runtime, "evalRoutes", &bundle, None).await;
                    let _ = reply.send(r);
                }
                Job::ProbeDefault { bundle, reply } => {
                    let r = call_harness(&mut runtime, "probeDefault", &bundle, None).await;
                    let _ = reply.send(r);
                }
                Job::Render { bundle, input, reply } => {
                    let r = call_harness(&mut runtime, "render", &bundle, Some(input)).await;
                    let _ = reply.send(r);
                }
            }
        }
    });
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
            "globalThis.__mesofact.{method}({url_json}, {})",
            serde_json::to_string(&v)?
        ),
        None => format!("globalThis.__mesofact.{method}({url_json})"),
    };
    let promise = runtime
        .execute_script("mesofact:call", script)
        .map_err(|e| anyhow!("{method} dispatch failed: {e}"))?;
    // `resolve()` registers the promise watcher and hands back an owned
    // future; with_event_loop_promise polls the event loop (module loads,
    // microtasks) until it settles.
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
