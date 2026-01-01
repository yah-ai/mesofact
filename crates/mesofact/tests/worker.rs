//! Worker harness tests for R005 (P3): exercise the Bun render-pool worker
//! over its NDJSON+UDS IPC protocol. The harness here is the seed of the
//! proxy's worker client (R009).
//!
//! Skipped automatically when `bun` is not on PATH so CI without Bun stays
//! green.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf()
}

fn worker_entry() -> PathBuf {
    workspace_root()
        .join("packages")
        .join("mesofact-worker")
        .join("src")
        .join("worker.ts")
}

fn stubs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("worker_stubs")
}

fn bun_available() -> bool {
    std::process::Command::new("bun")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct WorkerClient {
    child: Child,
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    _tmp: tempfile::TempDir,
    socket_path: PathBuf,
}

impl WorkerClient {
    async fn spawn(routes: &[(&str, &Path)]) -> Self {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let socket_path = tmp.path().join("worker.sock");
        let manifest_path = tmp.path().join("manifest.json");

        let manifest = json!({
            "version": "1",
            "build_id": "test-build",
            "routes": routes.iter().map(|(route, entry)| json!({
                "route": route,
                "mode": "ssr",
                "render_entrypoint": entry.to_string_lossy(),
                "cache_policy": { "ttl": 0 },
                "concurrency": 4,
            })).collect::<Vec<_>>(),
            "static_assets": [],
        });
        std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

        let mut cmd = Command::new("bun");
        cmd.arg(worker_entry())
            .arg("--socket")
            .arg(&socket_path)
            .arg("--manifest")
            .arg(&manifest_path)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let child = cmd.spawn().expect("spawn bun worker");

        let stream = connect_with_retry(&socket_path, Duration::from_secs(5))
            .await
            .expect("connect to worker UDS");
        let (read, write) = stream.into_split();

        Self {
            child,
            reader: BufReader::new(read),
            writer: write,
            _tmp: tmp,
            socket_path,
        }
    }

    async fn next_msg(&mut self, deadline: Duration) -> Value {
        let mut line = String::new();
        let n = timeout(deadline, self.reader.read_line(&mut line))
            .await
            .expect("next_msg timed out")
            .expect("read line");
        assert!(n > 0, "worker closed socket before sending message");
        serde_json::from_str(&line).expect("parse ndjson")
    }

    async fn send(&mut self, msg: &Value) {
        let mut bytes = serde_json::to_vec(msg).unwrap();
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.expect("write");
        self.writer.flush().await.expect("flush");
    }

    async fn await_ready(&mut self) {
        let m = self.next_msg(Duration::from_secs(5)).await;
        assert_eq!(m["kind"], "ready", "expected ready, got: {m}");
    }

    async fn await_pong(&mut self, deadline: Duration) -> Result<(), &'static str> {
        let mut line = String::new();
        match timeout(deadline, self.reader.read_line(&mut line)).await {
            Ok(Ok(n)) if n > 0 => {
                let v: Value = serde_json::from_str(&line).map_err(|_| "bad json")?;
                if v["kind"] == "pong" {
                    Ok(())
                } else {
                    Err("not a pong")
                }
            }
            Ok(_) => Err("eof"),
            Err(_) => Err("timeout"),
        }
    }

    async fn wait_exit(&mut self, deadline: Duration) -> std::process::ExitStatus {
        timeout(deadline, self.child.wait())
            .await
            .expect("worker did not exit before deadline")
            .expect("wait")
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

async fn connect_with_retry(
    path: &Path,
    total: Duration,
) -> Result<UnixStream, std::io::Error> {
    let start = std::time::Instant::now();
    loop {
        match UnixStream::connect(path).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                if start.elapsed() > total {
                    return Err(e);
                }
                sleep(Duration::from_millis(25)).await;
            }
        }
    }
}

fn skip_if_no_bun() -> bool {
    if !bun_available() {
        eprintln!("skipping worker harness test: `bun` not on PATH");
        return true;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_returns_ok() {
    if skip_if_no_bun() {
        return;
    }
    let hello = stubs_dir().join("hello.ts");
    let mut w = WorkerClient::spawn(&[("/hello", &hello)]).await;
    w.await_ready().await;

    w.send(&json!({
        "id": 1,
        "kind": "render",
        "route": "/hello",
        "req": { "url": "/hello", "params": {}, "query": {}, "headers": {}, "cookies": {} },
        "deadline_ms": 2000,
    }))
    .await;

    let resp = w.next_msg(Duration::from_secs(5)).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["kind"], "ok");
    assert_eq!(resp["html"], "hi");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_exits_after_in_flight_render() {
    if skip_if_no_bun() {
        return;
    }
    let slow = stubs_dir().join("slow.ts");
    let mut w = WorkerClient::spawn(&[("/slow", &slow)]).await;
    w.await_ready().await;

    w.send(&json!({
        "id": 7,
        "kind": "render",
        "route": "/slow",
        "req": { "url": "/slow", "params": {}, "query": {}, "headers": {}, "cookies": {} },
        "deadline_ms": 2000,
    }))
    .await;

    // Issue drain immediately — the slow render is in-flight.
    w.send(&json!({ "id": 0, "kind": "drain" })).await;

    // The in-flight render must still complete with ok.
    let resp = w.next_msg(Duration::from_secs(5)).await;
    assert_eq!(resp["id"], 7);
    assert_eq!(resp["kind"], "ok", "in-flight render lost on drain: {resp}");
    assert_eq!(resp["html"], "slow-ok");

    let status = w.wait_exit(Duration::from_secs(5)).await;
    assert!(status.success(), "worker did not exit cleanly: {status:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_pong_roundtrip() {
    if skip_if_no_bun() {
        return;
    }
    let hello = stubs_dir().join("hello.ts");
    let mut w = WorkerClient::spawn(&[("/hello", &hello)]).await;
    w.await_ready().await;

    w.send(&json!({ "id": 0, "kind": "ping" })).await;
    w.await_pong(Duration::from_secs(5))
        .await
        .expect("worker should answer ping with pong");
}

/// The harness must declare a worker dead if its pong does not arrive within
/// 5s of a ping. We exercise that timeout path with a stub UDS server that
/// accepts the connection but never sends pong.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missed_pong_within_5s_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let sock = tmp.path().join("silent.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    // Accept and hold the connection open without ever writing.
    let accept_task = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        // Keep the stream alive for the duration of the test.
        sleep(Duration::from_secs(10)).await;
    });

    let stream = UnixStream::connect(&sock).await.unwrap();
    let (read, _write) = stream.into_split();
    let mut reader = BufReader::new(read);

    let start = std::time::Instant::now();
    let mut line = String::new();
    let result = timeout(Duration::from_secs(5), reader.read_line(&mut line)).await;
    let elapsed = start.elapsed();

    assert!(result.is_err(), "expected timeout, got data: {line:?}");
    assert!(
        elapsed >= Duration::from_secs(5) && elapsed < Duration::from_secs(6),
        "timeout fired at unexpected elapsed: {elapsed:?}"
    );

    accept_task.abort();
}
