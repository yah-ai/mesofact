//! Mode 2 response cache — in-memory LRU keyed by the 7-input SHA-256
//! composition from `.yah/docs/architecture/mesofact.md` §"Cache-key
//! composition", with the fresh / stale (SWR) / expired state machine from
//! §"Mode 2 caching beyond TTL".
//!
//! Invalidation is generation-driven, not purge-driven: each source's current
//! generation token (sqlite file mtime, r2 Last-Modified, …) is folded into the
//! key (input 6), so a backend bump yields a different key and an automatic
//! miss on the next request — no reverse tag index needed in the hot path.

use lru::LruCache;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default LRU capacity (entries). Tunable via `ResponseCache::with_capacity`.
pub const DEFAULT_CAPACITY: usize = 4096;

/// The ordered inputs to the cache key. Maps are `BTreeMap` so iteration is
/// already sorted — input 3 (params), 4 (query), 5 (vary), and 6 (source
/// generations) all require deterministic key order.
#[derive(Debug, Clone)]
pub struct KeyInputs<'a> {
    /// 1. Whole-cache buster — changes every deploy.
    pub build_id: &'a str,
    /// 2. The route *pattern* (`/p/:id`), not the resolved URL.
    pub route_pattern: &'a str,
    /// 3. Resolved param map, keys sorted.
    pub params: &'a BTreeMap<String, String>,
    /// 4. Query string, keys sorted.
    pub query: &'a BTreeMap<String, String>,
    /// 5. `cache_policy.vary` header values, keyed by header name (sorted).
    pub vary: &'a BTreeMap<String, String>,
    /// 6. `source_name → generation` for every source in `source_reads`.
    pub source_generations: &'a BTreeMap<String, String>,
    /// 7. `user.id` (or `"_anon"`) — `Some` only when the route `requires` user.
    pub user_id: Option<&'a str>,
}

/// Compose the SHA-256 cache key as a lowercase hex string. Each field is fed
/// with a label and a record separator so component boundaries can't collide
/// (e.g. a param value can't impersonate a query key).
pub fn compose_key(inputs: &KeyInputs) -> String {
    let mut h = Sha256::new();
    feed(&mut h, b"build_id", inputs.build_id.as_bytes());
    feed(&mut h, b"route", inputs.route_pattern.as_bytes());
    feed_map(&mut h, b"params", inputs.params);
    feed_map(&mut h, b"query", inputs.query);
    feed_map(&mut h, b"vary", inputs.vary);
    feed_map(&mut h, b"gen", inputs.source_generations);
    if let Some(uid) = inputs.user_id {
        feed(&mut h, b"user", uid.as_bytes());
    }
    hex::encode(h.finalize())
}

fn feed(h: &mut Sha256, label: &[u8], value: &[u8]) {
    h.update(label);
    h.update([0u8]);
    h.update(value);
    h.update([0x1eu8]); // ASCII record separator
}

fn feed_map(h: &mut Sha256, label: &[u8], map: &BTreeMap<String, String>) {
    h.update(label);
    h.update([0x02u8]); // start-of-map
    for (k, v) in map {
        h.update(k.as_bytes());
        h.update([0x1fu8]); // unit separator between k and v
        h.update(v.as_bytes());
        h.update([0x1eu8]);
    }
    h.update([0x03u8]); // end-of-map
}

/// Freshness verdict for a stored entry at a given instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// `age < ttl` — serve as-is.
    Fresh,
    /// `ttl <= age < ttl + swr` — serve, kick off async re-render.
    Stale,
    /// `age >= ttl + swr` — synchronous miss.
    Expired,
}

impl CacheState {
    /// Metric/`X-Mesofact-Cache` label.
    pub fn label(self) -> &'static str {
        match self {
            CacheState::Fresh => "fresh",
            CacheState::Stale => "stale",
            CacheState::Expired => "expired",
        }
    }
}

/// A cached render result plus the TTL/SWR window it was stored under.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub status: u16,
    pub html: String,
    pub headers: BTreeMap<String, String>,
    pub ttl: Duration,
    pub swr: Duration,
    pub stored_at: Instant,
}

impl CacheEntry {
    pub fn state_at(&self, now: Instant) -> CacheState {
        let age = now.saturating_duration_since(self.stored_at);
        if age < self.ttl {
            CacheState::Fresh
        } else if age < self.ttl + self.swr {
            CacheState::Stale
        } else {
            CacheState::Expired
        }
    }

    pub fn state(&self) -> CacheState {
        self.state_at(Instant::now())
    }
}

/// Thread-safe LRU response cache. `get`/`insert` take `&self`; the `Mutex`
/// guards the `LruCache` (whose `get` needs `&mut` to bump recency).
pub struct ResponseCache {
    inner: Mutex<LruCache<String, CacheEntry>>,
}

impl ResponseCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(cap: usize) -> Self {
        let cap = NonZeroUsize::new(cap).unwrap_or(NonZeroUsize::new(1).unwrap());
        Self { inner: Mutex::new(LruCache::new(cap)) }
    }

    /// Look up an entry, bumping its recency. Returns a clone so the lock is
    /// released before the caller inspects state / rebuilds a response.
    pub fn get(&self, key: &str) -> Option<CacheEntry> {
        self.inner.lock().unwrap().get(key).cloned()
    }

    pub fn insert(&self, key: String, entry: CacheEntry) {
        self.inner.lock().unwrap().put(key, entry);
    }

    /// Drop a single key (used by an SWR refresh that produced an uncacheable
    /// result, e.g. a 5xx — see `should_cache`).
    pub fn remove(&self, key: &str) {
        self.inner.lock().unwrap().pop(key);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ResponseCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Caching policy by status class (§"Mode 2 caching beyond TTL"):
/// 2xx/3xx cache under `ttl`+`swr`; non-2xx (4xx) cache under `negative_ttl`
/// with no SWR window; 5xx is never cached. Returns `(ttl, swr)` to store
/// under, or `None` to skip caching.
pub fn cache_window(
    status: u16,
    ttl: Duration,
    swr: Duration,
    negative_ttl: Duration,
) -> Option<(Duration, Duration)> {
    if (500..600).contains(&status) {
        None
    } else if (200..400).contains(&status) {
        Some((ttl, swr))
    } else {
        // 4xx and other non-2xx: negative cache, no stale window.
        Some((negative_ttl, Duration::ZERO))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn base_inputs<'a>(
        build_id: &'a str,
        empty: &'a BTreeMap<String, String>,
    ) -> KeyInputs<'a> {
        KeyInputs {
            build_id,
            route_pattern: "/p/:id",
            params: empty,
            query: empty,
            vary: empty,
            source_generations: empty,
            user_id: None,
        }
    }

    #[test]
    fn key_is_deterministic_and_64_hex_chars() {
        let empty = map(&[]);
        let k1 = compose_key(&base_inputs("b1", &empty));
        let k2 = compose_key(&base_inputs("b1", &empty));
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64);
        assert!(k1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn build_id_busts_the_key() {
        let empty = map(&[]);
        assert_ne!(
            compose_key(&base_inputs("b1", &empty)),
            compose_key(&base_inputs("b2", &empty)),
        );
    }

    #[test]
    fn source_generation_bump_changes_the_key() {
        let empty = map(&[]);
        let g1 = map(&[("project_db", "100")]);
        let g2 = map(&[("project_db", "200")]);
        let mut a = base_inputs("b1", &empty);
        a.source_generations = &g1;
        let mut b = base_inputs("b1", &empty);
        b.source_generations = &g2;
        assert_ne!(compose_key(&a), compose_key(&b));
    }

    #[test]
    fn user_id_only_folds_in_when_present() {
        let empty = map(&[]);
        let anon = base_inputs("b1", &empty);
        let mut with_user = base_inputs("b1", &empty);
        with_user.user_id = Some("u42");
        assert_ne!(compose_key(&anon), compose_key(&with_user));
    }

    #[test]
    fn map_field_boundaries_do_not_collide() {
        // params {a: "b"} must not hash the same as params {} + query {a: "b"}.
        let empty = map(&[]);
        let ab = map(&[("a", "b")]);
        let mut as_param = base_inputs("b1", &empty);
        as_param.params = &ab;
        let mut as_query = base_inputs("b1", &empty);
        as_query.query = &ab;
        assert_ne!(compose_key(&as_param), compose_key(&as_query));
    }

    fn entry(ttl: u64, swr: u64, stored_ago: Duration) -> CacheEntry {
        CacheEntry {
            status: 200,
            html: "x".into(),
            headers: BTreeMap::new(),
            ttl: Duration::from_secs(ttl),
            swr: Duration::from_secs(swr),
            stored_at: Instant::now() - stored_ago,
        }
    }

    #[test]
    fn state_machine_fresh_stale_expired() {
        // ttl=60, swr=300 → fresh <60s, stale 60..360s, expired ≥360s.
        assert_eq!(entry(60, 300, Duration::from_secs(10)).state(), CacheState::Fresh);
        assert_eq!(entry(60, 300, Duration::from_secs(120)).state(), CacheState::Stale);
        assert_eq!(entry(60, 300, Duration::from_secs(400)).state(), CacheState::Expired);
    }

    #[test]
    fn zero_ttl_zero_swr_is_immediately_expired() {
        assert_eq!(entry(0, 0, Duration::from_millis(1)).state(), CacheState::Expired);
    }

    #[test]
    fn cache_window_classes() {
        let ttl = Duration::from_secs(60);
        let swr = Duration::from_secs(300);
        let neg = Duration::from_secs(10);
        // 2xx/3xx → (ttl, swr)
        assert_eq!(cache_window(200, ttl, swr, neg), Some((ttl, swr)));
        assert_eq!(cache_window(302, ttl, swr, neg), Some((ttl, swr)));
        // 4xx → negative cache, no swr
        assert_eq!(cache_window(404, ttl, swr, neg), Some((neg, Duration::ZERO)));
        // 5xx → never cached
        assert_eq!(cache_window(503, ttl, swr, neg), None);
    }

    #[test]
    fn lru_round_trip_and_recency_eviction() {
        let cache = ResponseCache::with_capacity(2);
        cache.insert("a".into(), entry(60, 0, Duration::ZERO));
        cache.insert("b".into(), entry(60, 0, Duration::ZERO));
        // Touch "a" so "b" becomes the LRU victim.
        assert!(cache.get("a").is_some());
        cache.insert("c".into(), entry(60, 0, Duration::ZERO));
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none(), "b should have been evicted");
        assert!(cache.get("c").is_some());
    }
}
