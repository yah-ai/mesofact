//! `op_mesofact_session` — hands the Rust-resolved user into the SSR isolate
//! (R750-F2). Identity is resolved outside this crate (mesofact-core's
//! `SessionResolver`, R556-B13) and arrives pre-resolved on
//! [`crate::DispatchRequest::user`] as an opaque `{id, attrs}` JSON value, so
//! mesofact-ssr never links cheers. The dispatch loop puts it in `OpState` for
//! the duration of one dispatch and clears it after; an isolate serves one
//! dispatch at a time, so the slot can never leak across requests.

use deno_core::{op2, OpState};
use serde_json::Value;

/// The current dispatch's user, or `None` when unauthenticated / outside a
/// dispatch.
#[derive(Default)]
pub(crate) struct DispatchSession(pub(crate) Option<Value>);

#[op2]
#[serde]
pub(crate) fn op_mesofact_session(state: &mut OpState) -> Option<Value> {
    state.try_borrow::<DispatchSession>().and_then(|s| s.0.clone())
}

deno_core::extension!(mesofact_session, ops = [op_mesofact_session]);
