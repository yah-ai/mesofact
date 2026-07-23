//! W3C Trace Context (`traceparent`) generation + parsing for the proxy.
//!
//! The proxy generates a `traceparent` per request, or continues an inbound one
//! (reusing its trace-id, minting a fresh span-id for the proxy span). The
//! resulting header is echoed on the response and passed to the Bun worker as
//! `req.ctx.trace` so a render log line can correlate with the proxy's. Adapter
//! child spans + OTLP export are post-MVP (see architecture §"Observability").
//!
//! Format (version 00): `00-<32 hex trace-id>-<16 hex parent-id>-<2 hex flags>`.
//!
//! IDs are minted from a SplitMix64 stream seeded by wall-clock nanos X'd with a
//! process-global counter — unique enough for trace correlation without pulling
//! in a CSPRNG (trace-ids are identifiers, not secrets).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceParent {
    /// 32 lowercase hex chars (128-bit), never all-zero.
    pub trace_id: String,
    /// 16 lowercase hex chars (64-bit) — the proxy's span id.
    pub span_id: String,
}

impl TraceParent {
    /// Continue an inbound `traceparent` (reuse trace-id, new proxy span-id) or
    /// mint a fresh one when the header is absent or malformed.
    pub fn incoming_or_new(header: Option<&str>) -> Self {
        if let Some(trace_id) = header.and_then(parse_trace_id) {
            return Self {
                trace_id,
                span_id: hex16(next_u64()),
            };
        }
        Self {
            trace_id: format!("{}{}", hex16(next_u64()), hex16(next_u64())),
            span_id: hex16(next_u64()),
        }
    }

    /// The `traceparent` header value. `01` = sampled.
    pub fn header_value(&self) -> String {
        format!("00-{}-{}-01", self.trace_id, self.span_id)
    }
}

/// Extract the trace-id from a `traceparent` header if it is well-formed:
/// version `00`, 32-hex trace-id (not all-zero), 16-hex parent-id (not
/// all-zero), 2-hex flags. Returns `None` on any deviation so a bad inbound
/// header falls back to a freshly-minted trace.
fn parse_trace_id(header: &str) -> Option<String> {
    let mut parts = header.trim().split('-');
    let version = parts.next()?;
    let trace_id = parts.next()?;
    let parent_id = parts.next()?;
    let flags = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if version != "00"
        || trace_id.len() != 32
        || parent_id.len() != 16
        || flags.len() != 2
        || !is_hex(trace_id)
        || !is_hex(parent_id)
        || !is_hex(flags)
        || trace_id.bytes().all(|b| b == b'0')
        || parent_id.bytes().all(|b| b == b'0')
    {
        return None;
    }
    Some(trace_id.to_ascii_lowercase())
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn hex16(v: u64) -> String {
    format!("{v:016x}")
}

/// Next pseudo-random u64 from a SplitMix64 step over (nanos ^ counter).
fn next_u64() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    splitmix64(nanos ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_traceparent_is_well_formed() {
        let tp = TraceParent::incoming_or_new(None);
        assert_eq!(tp.trace_id.len(), 32);
        assert_eq!(tp.span_id.len(), 16);
        assert!(is_hex(&tp.trace_id) && is_hex(&tp.span_id));
        let h = tp.header_value();
        assert!(h.starts_with("00-") && h.ends_with("-01"));
        // round-trips: parsing our own output yields the same trace-id.
        assert_eq!(parse_trace_id(&h), Some(tp.trace_id));
    }

    #[test]
    fn continues_inbound_trace_with_new_span() {
        let inbound = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let tp = TraceParent::incoming_or_new(Some(inbound));
        assert_eq!(tp.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        // Proxy mints its own span; it must not echo the inbound parent-id.
        assert_ne!(tp.span_id, "00f067aa0ba902b7");
    }

    #[test]
    fn malformed_inbound_falls_back_to_fresh() {
        for bad in [
            "garbage",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01", // version
            "00-tooshort-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01", // all-zero trace
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01", // all-zero parent
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7", // missing flags
        ] {
            let tp = TraceParent::incoming_or_new(Some(bad));
            assert_ne!(tp.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736", "bad={bad}");
            assert_eq!(tp.trace_id.len(), 32, "bad={bad}");
        }
    }

    #[test]
    fn ids_are_unique_across_calls() {
        let a = TraceParent::incoming_or_new(None);
        let b = TraceParent::incoming_or_new(None);
        assert_ne!(a.trace_id, b.trace_id);
        assert_ne!(a.span_id, b.span_id);
    }
}
