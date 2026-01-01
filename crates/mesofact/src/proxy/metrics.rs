//! Prometheus metrics for the proxy (`/metrics` exposition, scraped by warden).
//!
//! MVP metric set from the architecture's §"Observability":
//!
//! | Metric | Type | Labels |
//! |---|---|---|
//! | `mesofact_requests_total` | counter | `route, mode, status` |
//! | `mesofact_render_duration_seconds` | histogram | `route` |
//! | `mesofact_cache_total` | counter | `route, state` (fresh/stale/miss) |
//! | `mesofact_worker_pool` | gauge | `state` (ready/busy/restarting) |
//!
//! Hand-rolled rather than pulling in the `prometheus` crate — the surface is
//! four metrics and a text encoder, matching the project's lean-deps posture
//! (cf. the publisher's hand-rolled SigV4). Counters/gauges live behind a
//! `Mutex` keyed by label tuples; `render()` walks them into the 0.0.4 text
//! exposition format.
//!
//! @yah:ticket(R012-T3, "Observability: Prometheus /metrics exporter (requests/render_duration/cache/worker_pool) + W3C traceparent generate/accept + passthrough to worker via req.ctx.trace")
//! @yah:at(2026-05-26T16:15:53Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:phase(P10)
//! @yah:parent(R012)
//! @yah:handoff("Observability MVP shipped. (A) metrics.rs: hand-rolled Prometheus registry (no prometheus crate) — mesofact_requests_total{route,mode,status} counter, mesofact_render_duration_seconds{route} histogram (cumulative le-buckets 5ms..10s + sum/count), mesofact_cache_total{route,state} (fresh/stale/miss), mesofact_worker_pool{state} gauge (ready/busy/restarting). render(ready) emits 0.0.4 text exposition; label values escaped. (B) trace.rs: W3C traceparent — TraceParent::incoming_or_new() continues a valid inbound (reuse trace-id, mint new span) or generates fresh (SplitMix64 over nanos^counter, no CSPRNG dep); header_value() = '00-{trace}-{span}-01'; parse rejects bad version/length/all-zero. (C) router.rs: handle() refactored to a single funnel — generates traceparent, dispatches via a Plan enum, echoes 'traceparent' response header, records requests_total once (incl. <unmatched> 404s). dispatch_ssr/render_miss/spawn_refresh record cache_total + observe render_duration + inc/dec the busy gauge; build_render_request injects ctx.trace. New pub metrics_handler (GET /metrics, bypasses the manifest matcher). (D) worker_pool.rs: live_count() (ready gauge) + attach_metrics() + restarting_inc/dec around respawn. (E) mesofact-proxy.rs: one shared Arc<Metrics> wired into AppState + pool (initial + reload), /metrics route mounted. Tests: 27 lib unit (4 metrics + 4 trace), 3 new proxy integration (metrics scrape after a render shows requests/cache/render/ready; trace_echo.ts stub proves req.ctx.trace == echoed traceparent; inbound trace-id continued).")
//! @yah:verify("cargo test -p mesofact")
//! @yah:verify("cargo check --workspace")
//! @yah:cleanup("mesofact_worker_pool{state=ready} = pool.live_count() always equals n (dead workers are replaced in-place, not removed), so 'ready' is really pool size; a truly-live count would require per-worker health state. 'busy' (proxy-side in-flight renders) + 'restarting' are accurate.")
//! @yah:assumes("Worker logging req.ctx.trace on each render (the DoD 'trace flows proxy→worker log line') is left to the operator/worker; the proxy→worker passthrough is verified by trace_echo.ts returning ctx.trace. Adapter child spans + OTLP export are post-MVP per the design.")

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

/// Cumulative histogram upper bounds (seconds) for render duration. The
/// implicit `+Inf` bucket equals the total count.
const DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Per-route render-duration histogram. `buckets[i]` is the cumulative count of
/// observations `<= DURATION_BUCKETS[i]` (each observation bumps every bucket
/// whose bound it falls under, so the stored value is already cumulative).
struct Histogram {
    buckets: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Histogram {
    fn new() -> Self {
        Self {
            buckets: vec![0; DURATION_BUCKETS.len()],
            sum: 0.0,
            count: 0,
        }
    }

    fn observe(&mut self, secs: f64) {
        self.count += 1;
        self.sum += secs;
        for (i, bound) in DURATION_BUCKETS.iter().enumerate() {
            if secs <= *bound {
                self.buckets[i] += 1;
            }
        }
    }
}

/// Proxy metrics registry. Cheap to clone behind an `Arc`; all mutation goes
/// through interior `Mutex`/atomics so handlers can share one instance.
pub struct Metrics {
    /// (route, mode, status) → count.
    requests: Mutex<BTreeMap<(String, String, u16), u64>>,
    /// (route, state) → count, state ∈ {fresh, stale, miss}.
    cache: Mutex<BTreeMap<(String, String), u64>>,
    /// route → render-duration histogram.
    render: Mutex<BTreeMap<String, Histogram>>,
    /// In-flight renders (proxy-side) — the `busy` worker-pool gauge.
    render_inflight: AtomicI64,
    /// Workers currently being respawned — the `restarting` worker-pool gauge.
    restarting: AtomicI64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            requests: Mutex::new(BTreeMap::new()),
            cache: Mutex::new(BTreeMap::new()),
            render: Mutex::new(BTreeMap::new()),
            render_inflight: AtomicI64::new(0),
            restarting: AtomicI64::new(0),
        }
    }

    pub fn record_request(&self, route: &str, mode: &str, status: u16) {
        *self
            .requests
            .lock()
            .unwrap()
            .entry((route.to_string(), mode.to_string(), status))
            .or_insert(0) += 1;
    }

    pub fn record_cache(&self, route: &str, state: &str) {
        *self
            .cache
            .lock()
            .unwrap()
            .entry((route.to_string(), state.to_string()))
            .or_insert(0) += 1;
    }

    pub fn observe_render(&self, route: &str, secs: f64) {
        self.render
            .lock()
            .unwrap()
            .entry(route.to_string())
            .or_insert_with(Histogram::new)
            .observe(secs);
    }

    pub fn inflight_inc(&self) {
        self.render_inflight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inflight_dec(&self) {
        self.render_inflight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn restarting_inc(&self) {
        self.restarting.fetch_add(1, Ordering::Relaxed);
    }

    pub fn restarting_dec(&self) {
        self.restarting.fetch_sub(1, Ordering::Relaxed);
    }

    /// Render the Prometheus text exposition (format 0.0.4). `ready` is the
    /// current live worker count, queried from the pool at scrape time.
    pub fn render(&self, ready: usize) -> String {
        let mut out = String::new();

        out.push_str("# HELP mesofact_requests_total Total HTTP requests handled.\n");
        out.push_str("# TYPE mesofact_requests_total counter\n");
        for ((route, mode, status), count) in self.requests.lock().unwrap().iter() {
            out.push_str(&format!(
                "mesofact_requests_total{{route=\"{}\",mode=\"{}\",status=\"{}\"}} {}\n",
                esc(route),
                esc(mode),
                status,
                count,
            ));
        }

        out.push_str("# HELP mesofact_render_duration_seconds Render duration through the Bun pool.\n");
        out.push_str("# TYPE mesofact_render_duration_seconds histogram\n");
        for (route, hist) in self.render.lock().unwrap().iter() {
            let r = esc(route);
            for (i, bound) in DURATION_BUCKETS.iter().enumerate() {
                out.push_str(&format!(
                    "mesofact_render_duration_seconds_bucket{{route=\"{}\",le=\"{}\"}} {}\n",
                    r, bound, hist.buckets[i],
                ));
            }
            out.push_str(&format!(
                "mesofact_render_duration_seconds_bucket{{route=\"{}\",le=\"+Inf\"}} {}\n",
                r, hist.count,
            ));
            out.push_str(&format!(
                "mesofact_render_duration_seconds_sum{{route=\"{}\"}} {}\n",
                r, hist.sum,
            ));
            out.push_str(&format!(
                "mesofact_render_duration_seconds_count{{route=\"{}\"}} {}\n",
                r, hist.count,
            ));
        }

        out.push_str("# HELP mesofact_cache_total Mode 2 cache outcomes by state.\n");
        out.push_str("# TYPE mesofact_cache_total counter\n");
        for ((route, state), count) in self.cache.lock().unwrap().iter() {
            out.push_str(&format!(
                "mesofact_cache_total{{route=\"{}\",state=\"{}\"}} {}\n",
                esc(route),
                esc(state),
                count,
            ));
        }

        out.push_str("# HELP mesofact_worker_pool Worker pool size by state.\n");
        out.push_str("# TYPE mesofact_worker_pool gauge\n");
        out.push_str(&format!(
            "mesofact_worker_pool{{state=\"ready\"}} {}\n",
            ready,
        ));
        out.push_str(&format!(
            "mesofact_worker_pool{{state=\"busy\"}} {}\n",
            self.render_inflight.load(Ordering::Relaxed).max(0),
        ));
        out.push_str(&format!(
            "mesofact_worker_pool{{state=\"restarting\"}} {}\n",
            self.restarting.load(Ordering::Relaxed).max(0),
        ));

        out
    }
}

/// Escape a Prometheus label value: backslash, double-quote, and newline per
/// the exposition spec. Route patterns (`/`, `:param`) need no escaping but a
/// `vary`-derived or future label might.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_counter_groups_by_label_tuple() {
        let m = Metrics::new();
        m.record_request("/", "static", 200);
        m.record_request("/", "static", 200);
        m.record_request("/api", "ssr", 503);
        let out = m.render(2);
        assert!(out.contains("mesofact_requests_total{route=\"/\",mode=\"static\",status=\"200\"} 2"));
        assert!(out.contains("mesofact_requests_total{route=\"/api\",mode=\"ssr\",status=\"503\"} 1"));
    }

    #[test]
    fn histogram_buckets_are_cumulative() {
        let m = Metrics::new();
        m.observe_render("/api", 0.02); // <= 0.025, 0.05, ...
        m.observe_render("/api", 0.2); // <= 0.25, 0.5, ...
        let out = m.render(1);
        // le=0.025 catches the 0.02 sample only.
        assert!(out.contains("mesofact_render_duration_seconds_bucket{route=\"/api\",le=\"0.025\"} 1"));
        // le=0.5 catches both.
        assert!(out.contains("mesofact_render_duration_seconds_bucket{route=\"/api\",le=\"0.5\"} 2"));
        assert!(out.contains("mesofact_render_duration_seconds_bucket{route=\"/api\",le=\"+Inf\"} 2"));
        assert!(out.contains("mesofact_render_duration_seconds_count{route=\"/api\"} 2"));
    }

    #[test]
    fn cache_and_pool_gauges_render() {
        let m = Metrics::new();
        m.record_cache("/api", "miss");
        m.record_cache("/api", "fresh");
        m.inflight_inc();
        let out = m.render(4);
        assert!(out.contains("mesofact_cache_total{route=\"/api\",state=\"miss\"} 1"));
        assert!(out.contains("mesofact_cache_total{route=\"/api\",state=\"fresh\"} 1"));
        assert!(out.contains("mesofact_worker_pool{state=\"ready\"} 4"));
        assert!(out.contains("mesofact_worker_pool{state=\"busy\"} 1"));
        assert!(out.contains("mesofact_worker_pool{state=\"restarting\"} 0"));
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(esc("a\"b\\c"), "a\\\"b\\\\c");
    }
}
