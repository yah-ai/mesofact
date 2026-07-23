//! WorkerClient — UDS connection to one Bun render-pool worker.
//!
//! Wire protocol: NDJSON-framed, one JSON object per line. `id = 0` is
//! reserved for lifecycle messages (ready / ping / pong / drain).
//! See `.yah/docs/architecture/mesofact.md` §"IPC protocol".
//!
//! Concurrency note: all I/O is serialized through `io: Mutex<WorkerIo>`.
//! For P7 this is fine — Mode 2 renders are stubbed (501), so the only
//! callers are the watchdog (ping) and shutdown (drain). Refactor in P9
//! when concurrent renders share the socket.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};

const PONG_DEADLINE: Duration = Duration::from_secs(5);
const CONNECT_DEADLINE: Duration = Duration::from_secs(10);
const READY_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("pong not received within 5 s")]
    PongTimeout,
    #[error("worker closed the connection")]
    Closed,
    #[error("render error '{code}' (retryable={retryable})")]
    Render { code: String, retryable: bool },
    #[error("worker queue is full (503)")]
    QueueOverflow,
    #[error("worker is draining")]
    Draining,
}

#[derive(Debug, Clone)]
pub struct RenderResult {
    pub html: String,
    pub headers: HashMap<String, String>,
    pub cache_ttl: Option<u64>,
    pub cache_tags: Vec<String>,
}

struct WorkerIo {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

pub struct WorkerClient {
    io: Mutex<WorkerIo>,
    pub socket_path: PathBuf,
    child: Mutex<Child>,
}

impl WorkerClient {
    /// Spawn a Bun worker and connect over UDS. Returns after the worker sends
    /// its `ready` lifecycle message.
    pub async fn spawn(
        socket_path: PathBuf,
        manifest_path: &Path,
        worker_entry: &Path,
        config_path: Option<&Path>,
    ) -> Result<Self, WorkerError> {
        let mut cmd = Command::new("bun");
        cmd.arg(worker_entry)
            .arg("--socket")
            .arg(&socket_path)
            .arg("--manifest")
            .arg(manifest_path)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(cfg) = config_path {
            // Worker registers adapters from `mesofact.config.toml` at boot so a
            // render's `sqlite('db')` / `r2('assets')` resolves at request time.
            cmd.arg("--config").arg(cfg);
        }

        let child = cmd.spawn()?;
        let stream = connect_with_retry(&socket_path, CONNECT_DEADLINE).await?;
        let (r, w) = stream.into_split();

        let client = Self {
            io: Mutex::new(WorkerIo {
                reader: BufReader::new(r),
                writer: w,
            }),
            socket_path,
            child: Mutex::new(child),
        };

        let msg = client.recv_with_timeout(READY_DEADLINE).await?;
        if msg.get("kind").and_then(|v| v.as_str()) != Some("ready") {
            return Err(WorkerError::Io(std::io::Error::other(format!(
                "expected ready, got {msg}"
            ))));
        }
        Ok(client)
    }

    /// Send a ping and wait up to 5 s for a pong.
    pub async fn ping(&self) -> Result<(), WorkerError> {
        let mut io = self.io.lock().await;
        send(&mut io.writer, &json!({ "id": 0, "kind": "ping" })).await?;

        timeout(PONG_DEADLINE, async {
            loop {
                let v = recv(&mut io.reader).await?;
                if v.get("kind").and_then(|k| k.as_str()) == Some("pong") {
                    return Ok(());
                }
            }
        })
        .await
        .map_err(|_| WorkerError::PongTimeout)?
    }

    /// Ask the worker to finish in-flight renders and then exit gracefully.
    pub async fn drain(&self) -> Result<(), WorkerError> {
        let mut io = self.io.lock().await;
        send(&mut io.writer, &json!({ "id": 0, "kind": "drain" })).await
    }

    /// Invoke render on the worker (used by Mode 2 in P9+; stubbed at 501 in P7).
    pub async fn render(
        &self,
        id: u32,
        route: &str,
        req: Value,
        deadline_ms: u64,
    ) -> Result<RenderResult, WorkerError> {
        let msg = json!({
            "id": id,
            "kind": "render",
            "route": route,
            "req": req,
            "deadline_ms": deadline_ms,
        });
        let mut io = self.io.lock().await;
        send(&mut io.writer, &msg).await?;

        loop {
            let v = recv(&mut io.reader).await?;
            if v.get("id").and_then(|i| i.as_u64()) != Some(u64::from(id)) {
                continue;
            }
            return match v.get("kind").and_then(|k| k.as_str()) {
                Some("ok") => Ok(RenderResult {
                    html: v["html"].as_str().unwrap_or("").to_string(),
                    headers: serde_json::from_value(v["headers"].clone()).unwrap_or_default(),
                    cache_ttl: v["cache"]["ttl"].as_u64(),
                    cache_tags: serde_json::from_value(v["cache"]["tags"].clone())
                        .unwrap_or_default(),
                }),
                Some("err") => {
                    let code = v["error"]["code"]
                        .as_str()
                        .unwrap_or("render_failed")
                        .to_string();
                    let retryable = v["error"]["retryable"].as_bool().unwrap_or(false);
                    Err(match code.as_str() {
                        "queue_overflow" => WorkerError::QueueOverflow,
                        "draining" => WorkerError::Draining,
                        _ => WorkerError::Render { code, retryable },
                    })
                }
                _ => Err(WorkerError::Io(std::io::Error::other(
                    "unexpected message kind",
                ))),
            };
        }
    }

    /// Kill the worker process immediately (no graceful drain).
    pub async fn kill(&self) -> std::io::Result<()> {
        self.child.lock().await.kill().await
    }

    /// Wait for the worker process to exit.
    pub async fn wait(&self) -> std::io::Result<std::process::ExitStatus> {
        self.child.lock().await.wait().await
    }

    async fn recv_with_timeout(&self, dur: Duration) -> Result<Value, WorkerError> {
        let mut io = self.io.lock().await;
        timeout(dur, recv(&mut io.reader))
            .await
            .map_err(|_| WorkerError::PongTimeout)?
    }
}

async fn send(writer: &mut OwnedWriteHalf, msg: &Value) -> Result<(), WorkerError> {
    let mut b = serde_json::to_vec(msg)?;
    b.push(b'\n');
    writer.write_all(&b).await?;
    writer.flush().await?;
    Ok(())
}

async fn recv(reader: &mut BufReader<OwnedReadHalf>) -> Result<Value, WorkerError> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Err(WorkerError::Closed);
    }
    Ok(serde_json::from_str(&line)?)
}

async fn connect_with_retry(path: &Path, deadline: Duration) -> Result<UnixStream, WorkerError> {
    let start = std::time::Instant::now();
    loop {
        match UnixStream::connect(path).await {
            Ok(s) => return Ok(s),
            Err(e) if start.elapsed() >= deadline => return Err(e.into()),
            Err(_) => sleep(Duration::from_millis(50)).await,
        }
    }
}
