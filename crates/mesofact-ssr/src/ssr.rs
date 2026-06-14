//! Long-lived dev-tier SSR runtime (W174 pillar 4 / R449-F2).
//!
//! One V8 isolate hosts every `mode:"ssr"` route's Fetch handler. Routes are
//! pre-loaded at construction (cold-import cost paid once), then `dispatch`
//! re-uses the loaded handlers for each request. Replaces the `bun run`
//! subprocess + reverse-proxy hop that R434-F3 originally shipped.
//!
//! `JsRuntime` is `!Send`, so the runtime owns a dedicated thread with a
//! current-thread tokio runtime; callers talk to it through a small Send
//! handle. Jobs flow over an mpsc channel — `register` blocks the caller
//! during cold-import; `dispatch` blocks the caller until the handler's
//! Response is fully realised. Concurrency lives at the axum layer above —
//! the isolate is single-threaded by V8 design.

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
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

const SSR_BOOTSTRAP: &str = include_str!("../js/ssr_bootstrap.js");
const SSR_HARNESS: &str = include_str!("../js/ssr_harness.js");
const HARNESS_SPECIFIER: &str = "mesofact-ssr:harness";

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
    pub fn start() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let thread = std::thread::Builder::new()
            .name("mesofact-ssr".into())
            .spawn(move || run_isolate(rx, ready_tx))
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

/// Module loader: file-system ESM plus the embedded harness module.
struct SsrModuleLoader;

impl ModuleLoader for SsrModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: deno_core::ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
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
            let code: String = if spec.as_str() == HARNESS_SPECIFIER {
                SSR_HARNESS.to_string()
            } else {
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

fn run_isolate(rx: mpsc::Receiver<Job>, ready: mpsc::Sender<Result<()>>) {
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
                    let r = call_harness(&mut runtime, "dispatch", &bundle, Some(input)).await;
                    let r = r.and_then(|v| {
                        serde_json::from_value::<DispatchResponse>(v)
                            .map_err(|e| anyhow!("deserialising SSR response: {e}"))
                    });
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

        let rt = SsrRuntime::start().expect("ssr runtime starts");
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
}
