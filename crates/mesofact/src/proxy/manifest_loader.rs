//! Manifest loading: boot from a local file, plus SIGHUP-triggered reload and
//! a 30-second heartbeat poll. Both signals push a new `Arc<Manifest>` through
//! a `tokio::sync::watch` channel consumed by the router and worker pool.
//!
//! Only file-based manifests are supported in P7. HTTP manifest fetching
//! (proxy reading directly from R2) is deferred to a later phase.

use crate::manifest::Manifest;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;
use tracing::{info, warn};

const HEARTBEAT: Duration = Duration::from_secs(30);

/// Load the manifest from a local file path.
pub async fn load_from_file(path: &Path) -> std::io::Result<Manifest> {
    let bytes = tokio::fs::read(path).await?;
    serde_json::from_slice(&bytes).map_err(|e| std::io::Error::other(e.to_string()))
}

/// Start a background task that watches `path` for changes, triggered by
/// SIGHUP or the 30-second heartbeat. New manifests are sent on `tx`.
///
/// Returns the sender so callers can also trigger reloads programmatically
/// (useful in tests without needing to send actual UNIX signals).
pub fn watch_manifest(
    path: PathBuf,
    tx: watch::Sender<Arc<Manifest>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP handler");
        let mut ticker = tokio::time::interval(HEARTBEAT);
        ticker.tick().await; // skip immediate first tick

        loop {
            tokio::select! {
                _ = sighup.recv() => {
                    info!("SIGHUP received — reloading manifest");
                    reload_once(&path, &tx).await;
                }
                _ = ticker.tick() => {
                    reload_once(&path, &tx).await;
                }
            }
        }
    })
}

/// Reload the manifest from `path`. If it parses and validates structurally,
/// send it on `tx`. On error, keep the current manifest live and log the error.
pub async fn reload_once(path: &Path, tx: &watch::Sender<Arc<Manifest>>) {
    match load_from_file(path).await {
        Ok(m) => {
            let new_version = m.build_id.clone();
            let old_version = tx.borrow().build_id.clone();
            if new_version == old_version {
                return; // no change
            }
            info!("manifest updated: {old_version} → {new_version}");
            tx.send_replace(Arc::new(m));
        }
        Err(e) => {
            warn!("manifest reload failed (keeping current): {e}");
        }
    }
}
