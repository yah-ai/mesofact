//! R637 — measure how much of the `fetch`/`Request`/`Response` surface SSR
//! actually exercises, empirically, against real built bundles.
//!
//! Boots the same isolate `SsrRuntime` boots (same extensions, same
//! `ssr_bootstrap.js`, same `ssr_harness.js`), inserts an instrumentation
//! script between bootstrap and harness, then registers + dispatches a battery
//! of requests at every bundle passed on the command line. Prints the recorded
//! hit-list as JSON.
//!
//! ```text
//! cargo run -p mesofact-ssr --example fetch_surface_probe -- \
//!     /abs/path/to/app/yah/web/marketing/dist/server/api_issues.js \
//!     /abs/path/to/app/yah/web/analytics/dist/server/index.js
//! ```
//!
//! A stub upstream HTTP server is started on 127.0.0.1:<ephemeral> and exported
//! as `process.env.ISSUE_TRACKER_URL`, so the marketing route's outbound
//! `fetch()` success path (upstream body/status/headers pass-through) runs for
//! real rather than only its connection-refused branch.

use anyhow::{anyhow, Context, Result};
use deno_core::error::ModuleLoaderError;
use deno_core::{
    resolve_import, JsRuntime, ModuleLoadResponse, ModuleLoader, ModuleSource, ModuleSourceCode,
    ModuleSpecifier, ModuleType, PollEventLoopOptions, RuntimeOptions,
};
use deno_permissions::PermissionsContainer;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

const SSR_BOOTSTRAP: &str = include_str!("../js/ssr_bootstrap.js");
const SSR_HARNESS: &str = include_str!("../js/ssr_harness.js");
const PROBE: &str = include_str!("fetch_surface_probe.js");
const RUNTIME_PURE: &str = include_str!("../js/runtime_shim.js");
const SSR_RUNTIME_SHIM: &str = include_str!("../js/ssr_runtime_shim.js");
const HARNESS_SPECIFIER: &str = "mesofact-ssr:harness";
const RUNTIME_SPECIFIER: &str = "mesofact-ssr:runtime";
const RUNTIME_PURE_SPECIFIER: &str = "mesofact:runtime-pure";

/// Mirror of `ssr::SsrModuleLoader` (private to the crate).
struct ProbeModuleLoader;

impl ModuleLoader for ProbeModuleLoader {
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
                            "only file:// modules load here (got {spec})"
                        ))
                    })?;
                    std::fs::read_to_string(&path).map_err(|e| {
                        ModuleLoaderError::generic(format!("failed reading {}: {e}", path.display()))
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

/// Canned upstream so the outbound-`fetch` success path is exercised, not just
/// the connection-refused branch. Serves every request the same 201.
fn start_stub_upstream() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding stub upstream")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Read the request head, then the body if content-length says so.
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(s) => s,
                Err(_) => continue,
            });
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            if len > 0 {
                let mut body = vec![0u8; len];
                let _ = reader.read_exact(&mut body);
            }
            let body = br#"{"id":"iss-probe-1","status":"open"}"#;
            let head = format!(
                "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\nx-probe-upstream: yes\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    Ok(port)
}

fn build_runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(ProbeModuleLoader)),
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

/// The request battery every bundle gets. Route code branches on method,
/// content-type and body validity, so a single GET would under-report.
fn battery(base: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "GET",
            json!({ "method": "GET", "url": format!("{base}/"), "headers": [["accept", "text/html"], ["cookie", "sid=probe"]], "body": Value::Null }),
        ),
        (
            "POST json valid",
            json!({ "method": "POST", "url": format!("{base}/"), "headers": [["content-type", "application/json"]], "body": Value::Array(br#"{"title":"probe issue","kind":"bug"}"#.iter().map(|b| json!(b)).collect()) }),
        ),
        (
            "POST wrong content-type",
            json!({ "method": "POST", "url": format!("{base}/"), "headers": [["content-type", "text/plain"]], "body": Value::Array(b"hello".iter().map(|b| json!(b)).collect()) }),
        ),
        (
            "POST malformed json",
            json!({ "method": "POST", "url": format!("{base}/"), "headers": [["content-type", "application/json"]], "body": Value::Array(b"{not json".iter().map(|b| json!(b)).collect()) }),
        ),
        (
            "POST json wrong shape",
            json!({ "method": "POST", "url": format!("{base}/"), "headers": [["content-type", "application/json"]], "body": Value::Array(br#"{"nope":1}"#.iter().map(|b| json!(b)).collect()) }),
        ),
        (
            "HEAD",
            json!({ "method": "HEAD", "url": format!("{base}/"), "headers": [], "body": Value::Null }),
        ),
    ]
}

async fn eval(runtime: &mut JsRuntime, name: &'static str, src: String) -> Result<Value> {
    let promise = runtime
        .execute_script(name, src)
        .map_err(|e| anyhow!("{name} threw: {e}"))?;
    let watcher = runtime.resolve(promise);
    let global = runtime
        .with_event_loop_promise(watcher, PollEventLoopOptions::default())
        .await
        .map_err(|e| anyhow!("{name} rejected: {e}"))?;
    deno_core::scope!(scope, runtime);
    let local = deno_core::v8::Local::new(scope, global);
    deno_core::serde_v8::from_v8::<Value>(scope, local)
        .map_err(|e| anyhow!("{name} returned unserializable value: {e}"))
}

fn main() -> Result<()> {
    let bundles: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if bundles.is_empty() {
        return Err(anyhow!(
            "usage: fetch_surface_probe <absolute path to dist/server/*.js> ..."
        ));
    }
    let port = start_stub_upstream()?;

    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    tokio_rt.block_on(async move {
        let mut runtime = build_runtime();
        runtime
            .op_state()
            .borrow_mut()
            .put(PermissionsContainer::allow_all(Arc::new(
                deno_permissions::RuntimePermissionDescriptorParser::new(
                    sys_traits::impls::RealSys,
                ),
            )));

        runtime
            .execute_script("mesofact-ssr:bootstrap", SSR_BOOTSTRAP)
            .map_err(|e| anyhow!("bootstrap failed: {e}"))?;
        // Route code reads this; point it at the stub so the fetch success path runs.
        runtime
            .execute_script(
                "mesofact-ssr:env",
                format!("globalThis.process.env.ISSUE_TRACKER_URL = 'http://127.0.0.1:{port}';"),
            )
            .map_err(|e| anyhow!("env injection failed: {e}"))?;
        runtime
            .execute_script("mesofact-ssr:probe", PROBE)
            .map_err(|e| anyhow!("probe install failed: {e}"))?;

        let harness_spec = ModuleSpecifier::parse(HARNESS_SPECIFIER).unwrap();
        let id = runtime.load_side_es_module(&harness_spec).await?;
        let receiver = runtime.mod_evaluate(id);
        runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await?;
        receiver.await?;

        let mut report = Vec::new();
        for bundle in &bundles {
            let abs = std::fs::canonicalize(bundle)
                .with_context(|| format!("bundle not found: {}", bundle.display()))?;
            let url = ModuleSpecifier::from_file_path(&abs)
                .map_err(|()| anyhow!("not an absolute path: {}", abs.display()))?;
            let url_json = serde_json::to_string(url.as_str())?;
            let label = short_label(&abs);

            runtime
                .execute_script(
                    "mesofact-ssr:probe-phase",
                    format!("globalThis.__probe.setPhase('register:{label}')"),
                )
                .map_err(|e| anyhow!("phase set failed: {e}"))?;
            eval(
                &mut runtime,
                "mesofact-ssr:register",
                format!("globalThis.__mesofact_ssr.register({url_json})"),
            )
            .await
            .with_context(|| format!("registering {label}"))?;

            for (name, req) in battery("http://probe.local") {
                runtime
                    .execute_script(
                        "mesofact-ssr:probe-phase",
                        format!("globalThis.__probe.setPhase('dispatch:{label}:{name}')"),
                    )
                    .map_err(|e| anyhow!("phase set failed: {e}"))?;
                let res = eval(
                    &mut runtime,
                    "mesofact-ssr:dispatch",
                    format!(
                        "globalThis.__mesofact_ssr.dispatch({url_json}, {}).then(r => ({{ status: r.status, headers: r.headers, bytes: r.body.length }}))",
                        serde_json::to_string(&req)?
                    ),
                )
                .await;
                report.push(json!({
                    "bundle": label,
                    "request": name,
                    "outcome": match &res {
                        Ok(v) => v.clone(),
                        Err(e) => json!({ "error": e.to_string() }),
                    },
                }));
            }
        }

        runtime
            .execute_script(
                "mesofact-ssr:probe-phase",
                "globalThis.__probe.setPhase('done')",
            )
            .map_err(|e| anyhow!("phase set failed: {e}"))?;
        let hits = eval(
            &mut runtime,
            "mesofact-ssr:dump",
            "Promise.resolve(globalThis.__probe.dump())".to_string(),
        )
        .await?;

        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "dispatches": report, "hits": hits }))?
        );
        Ok::<(), anyhow::Error>(())
    })
}

fn short_label(p: &Path) -> String {
    let mut parts: Vec<&str> = p
        .iter()
        .rev()
        .take(4)
        .filter_map(|s| s.to_str())
        .collect::<Vec<_>>();
    parts.reverse();
    parts.join("/")
}
