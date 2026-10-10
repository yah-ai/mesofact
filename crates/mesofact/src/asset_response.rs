//! Cache validators, declared immutability and precompressed encodings for
//! the static tier (MFT-R825-F1).
//!
//! Until this module, a disk hit was answered with `Content-Type` and the
//! bytes, nothing else: no `ETag`, no `Cache-Control`, identity encoding
//! whatever the client accepted. A browser could not even *revalidate*, so
//! every visit to noisetable.com/app re-downloaded a 35 MB wasm (measured
//! 2026-10-10 against serve 0.8.41).
//!
//! Every static response now carries:
//!
//! - **`ETag`** = the file's sha256 — the digest the build's asset index
//!   recorded, or one computed here (once per file version) when the tree was
//!   never finalized. `If-None-Match` that matches gets a bodiless 304.
//! - **`Cache-Control`** = `public, max-age=31536000, immutable` for a file the
//!   component *declared* content-addressed (`[build] immutable` in its
//!   `mesofact.config.toml`, carried by the index), `no-cache` for everything
//!   else.
//!   Never inferred from a file name here: see
//!   [`yah_mesofact_bundle::assets`] for why.
//! - **`Content-Encoding`** from a precompressed `<file>.br` / `<file>.gz`
//!   written at publish time, chosen against `Accept-Encoding`, with
//!   `Vary: Accept-Encoding` on every representation of a file that has
//!   variants. Nothing is compressed on the request path.
//!
//! **Which index governs a file** is answered by walking up from the file's
//! directory to the served root and taking the first index with an entry for
//! it. That is what makes a merged multi-component bundle work with no mount
//! awareness here: a mounted component's index staged at
//! `html/<mount>/.mesofact-assets.json` with the component, keyed relative to
//! that directory.
//!
//! Precedence with the header layers outside this handler is unchanged: a
//! route's declared `cache_policy` ([`crate::cache_headers`]) and then the
//! domain manifest's route headers ([`crate::route_headers`]) still overwrite
//! `Cache-Control` on the way out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use yah_mesofact_bundle::assets::{
    etag_for, sha256_hex, AssetEntry, AssetIndex, Encoding, ASSET_INDEX_FILE,
    CACHE_CONTROL_REVALIDATE,
};

/// Past this many cached entries a map is cleared rather than evicted
/// piecemeal. Entries are cheap to rebuild (one stat + one parse, or one hash),
/// and a served tree with more distinct files than this is not one this tier
/// is sized for.
const CACHE_CAP: usize = 4096;

/// Identity of one version of a file on disk: when either half moves, every
/// cached fact about the old version is dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    len: u64,
    mtime: Option<SystemTime>,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self { len: meta.len(), mtime: meta.modified().ok() }
    }
}

/// Per-router memo of parsed asset indexes and on-demand hashes. Both are
/// keyed by absolute path and validated against the file's [`Stamp`] on every
/// use, so a dist pointer swap (a new path) or an in-place rebuild (a new
/// stamp) can never serve a stale fact.
#[derive(Default)]
pub(crate) struct AssetCache {
    indexes: Mutex<HashMap<PathBuf, (Stamp, Option<Arc<AssetIndex>>)>>,
    hashes: Mutex<HashMap<PathBuf, (Stamp, Arc<str>)>>,
}

impl AssetCache {
    /// The entry governing `target`: the nearest index at or above its
    /// directory (bounded by `root`) that lists it with its current size. An
    /// entry whose size no longer matches describes an older file and is
    /// ignored.
    async fn entry_for(&self, root: &Path, target: &Path, len: u64) -> Option<AssetEntry> {
        let start = target.parent()?;
        for dir in start.ancestors().take_while(|d| d.starts_with(root)) {
            let index_path = dir.join(ASSET_INDEX_FILE);
            let Ok(meta) = tokio::fs::metadata(&index_path).await else {
                continue;
            };
            let Some(index) = self.index_at(&index_path, Stamp::of(&meta)).await else {
                continue;
            };
            let Ok(rel) = target.strip_prefix(dir) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if let Some(entry) = index.assets.get(&rel).filter(|e| e.size == len) {
                return Some(entry.clone());
            }
        }
        None
    }

    async fn index_at(&self, path: &Path, stamp: Stamp) -> Option<Arc<AssetIndex>> {
        if let Some((s, index)) = self.indexes.lock().expect("asset cache poisoned").get(path) {
            if *s == stamp {
                return index.clone();
            }
        }
        // Unreadable or unparseable is cached too (as `None`), so a corrupt
        // index costs one parse per version, not one per request.
        let parsed = tokio::fs::read(path)
            .await
            .ok()
            .and_then(|raw| AssetIndex::parse(&raw))
            .map(Arc::new);
        let mut map = self.indexes.lock().expect("asset cache poisoned");
        if map.len() >= CACHE_CAP {
            map.clear();
        }
        map.insert(path.to_path_buf(), (stamp, parsed.clone()));
        parsed
    }

    fn cached_hash(&self, path: &Path, stamp: Stamp) -> Option<Arc<str>> {
        let map = self.hashes.lock().expect("asset cache poisoned");
        map.get(path).filter(|(s, _)| *s == stamp).map(|(_, h)| h.clone())
    }

    fn remember_hash(&self, path: &Path, stamp: Stamp, sha: Arc<str>) {
        let mut map = self.hashes.lock().expect("asset cache poisoned");
        if map.len() >= CACHE_CAP {
            map.clear();
        }
        map.insert(path.to_path_buf(), (stamp, sha));
    }
}

/// Answer a request for the regular file `target` under served `root`.
/// `None` when `target` is not a servable file (absent, a directory, or the
/// asset index itself), so the caller moves on to its next candidate.
pub(crate) async fn respond(
    cache: &AssetCache,
    root: &Path,
    target: &Path,
    content_type: &'static str,
    req: &HeaderMap,
) -> Option<Response> {
    if target.file_name().is_some_and(|n| n == ASSET_INDEX_FILE) {
        return None;
    }
    let meta = tokio::fs::metadata(target).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let stamp = Stamp::of(&meta);

    if let Some(entry) = cache.entry_for(root, target, stamp.len).await {
        return Some(respond_indexed(&entry, target, content_type, req).await);
    }

    // Unindexed — a tree no publish step finalized (`mesofact-dev`, a
    // hand-run `mesofact serve --workload`), or a file added after it was.
    // Still validated: hash once per file version, revalidate every use.
    let (sha, body) = match cache.cached_hash(target, stamp) {
        Some(sha) => (sha, None),
        None => {
            let bytes = tokio::fs::read(target).await.ok()?;
            let (sha, bytes) = tokio::task::spawn_blocking(move || (sha256_hex(&bytes), bytes))
                .await
                .ok()?;
            let sha: Arc<str> = sha.into();
            cache.remember_hash(target, stamp, sha.clone());
            (sha, Some(bytes))
        }
    };
    let etag = etag_for(&sha, None);
    let mut headers = validator_headers(&etag, CACHE_CONTROL_REVALIDATE, false);
    if if_none_match(req, &etag) {
        return Some((StatusCode::NOT_MODIFIED, headers).into_response());
    }
    let body = match body {
        Some(b) => b,
        None => tokio::fs::read(target).await.ok()?,
    };
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    Some((headers, body).into_response())
}

async fn respond_indexed(
    entry: &AssetEntry,
    target: &Path,
    content_type: &'static str,
    req: &HeaderMap,
) -> Response {
    let vary = !entry.encodings.is_empty();
    let mut chosen = negotiate(req, &entry.encodings);
    let mut body = None;
    if let Some(enc) = chosen {
        let mut variant = target.as_os_str().to_owned();
        variant.push(enc.suffix());
        match tokio::fs::read(PathBuf::from(variant)).await {
            Ok(b) => body = Some(b),
            // The index promised a variant the tree no longer has: serve
            // identity rather than fail — correct bytes, just bigger.
            Err(_) => chosen = None,
        }
    }

    let etag = entry.etag(chosen);
    let mut headers = validator_headers(&etag, entry.cache_control(), vary);
    if if_none_match(req, &etag) {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    let body = match body {
        Some(b) => b,
        None => match tokio::fs::read(target).await {
            Ok(b) => b,
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Some(enc) = chosen {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc.token()));
    }
    (headers, body).into_response()
}

/// The headers a 200 and its 304 must share (RFC 9110 §15.4.5: a 304 carries
/// the `ETag`, `Cache-Control` and `Vary` the 200 would have).
fn validator_headers(etag: &str, cache_control: &'static str, vary: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(etag) {
        h.insert(header::ETAG, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
    if vary {
        h.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    h
}

/// Pick the best encoding among `available` the request accepts. No
/// `Accept-Encoding` at all ⇒ identity: RFC 9110 permits any coding then, but
/// a client that sent nothing (curl without `--compressed`, a health probe)
/// expects bytes it can read. Highest q wins; ties go to `available`'s order
/// (brotli first); `q=0` refuses a coding outright, including via `*`.
pub(crate) fn negotiate(req: &HeaderMap, available: &[Encoding]) -> Option<Encoding> {
    if available.is_empty() {
        return None;
    }
    let mut q_by_token: HashMap<String, f32> = HashMap::new();
    for value in req.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for item in value.split(',') {
            let mut parts = item.split(';');
            let token = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            if token.is_empty() {
                continue;
            }
            let q = parts
                .filter_map(|p| p.trim().strip_prefix("q=").or_else(|| p.trim().strip_prefix("Q=")))
                .find_map(|v| v.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            q_by_token.insert(token, q);
        }
    }
    let star = q_by_token.get("*").copied();
    let mut best: Option<(Encoding, f32)> = None;
    for &enc in available {
        let q = q_by_token.get(enc.token()).copied().or(star).unwrap_or(0.0);
        if q > 0.0 && best.is_none_or(|(_, bq)| q > bq) {
            best = Some((enc, q));
        }
    }
    best.map(|(e, _)| e)
}

/// `If-None-Match` names `etag` (weak comparison, RFC 9110 §13.1.2) or is `*`.
pub(crate) fn if_none_match(req: &HeaderMap, etag: &str) -> bool {
    req.get_all(header::IF_NONE_MATCH).iter().any(|value| {
        value.to_str().is_ok_and(|v| {
            v.split(',').map(str::trim).any(|tag| {
                tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(v).unwrap());
        h
    }

    const BOTH: &[Encoding] = &[Encoding::Br, Encoding::Gzip];

    #[test]
    fn negotiation_prefers_brotli_honours_q_and_defaults_to_identity() {
        assert_eq!(negotiate(&HeaderMap::new(), BOTH), None, "no header ⇒ identity");
        assert_eq!(negotiate(&accept("br, gzip"), BOTH), Some(Encoding::Br));
        assert_eq!(negotiate(&accept("gzip, deflate"), BOTH), Some(Encoding::Gzip));
        assert_eq!(negotiate(&accept("gzip;q=1.0, br;q=0.5"), BOTH), Some(Encoding::Gzip));
        assert_eq!(negotiate(&accept("br;q=0, gzip"), BOTH), Some(Encoding::Gzip));
        assert_eq!(negotiate(&accept("*"), BOTH), Some(Encoding::Br));
        assert_eq!(negotiate(&accept("*;q=0"), BOTH), None);
        assert_eq!(negotiate(&accept("identity"), BOTH), None);
        assert_eq!(negotiate(&accept("br"), &[Encoding::Gzip]), None, "only what exists");
    }

    #[test]
    fn if_none_match_uses_weak_comparison_and_lists() {
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"x\", W/\"abc\""));
        assert!(if_none_match(&h, "\"abc\""));
        assert!(!if_none_match(&h, "\"abd\""));
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(if_none_match(&h, "\"anything\""));
        assert!(!if_none_match(&HeaderMap::new(), "\"abc\""));
    }
}
