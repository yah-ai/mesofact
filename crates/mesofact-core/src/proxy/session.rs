//! Auth & session contract — the Rust proxy resolves `req.user` before render.
//!
//! mesofact decodes the session cookie with **cheers-core**'s [`Codec`] (R009 /
//! P11). The MVP placeholder HMAC implementation that used to live here was
//! replaced by a path dependency on `cheers-core`, the shared auth contract used
//! by every yah product. `CookieSessionResolver` reads a configurable cookie
//! (default `mesofact_session`), hands the raw token to a `cheers_core::Codec`
//! for verification, and maps the verified [`Claims`] onto mesofact's
//! render-facing [`User`].
//!
//! **Default codec:** [`PasetoV4Codec`] (PASETO v4.local — encrypted *and*
//! authenticated), the cheers-recommended default. mesofact is the SSR *origin*
//! (it holds the symmetric key and verifies server-side; the render worker never
//! sees the token, only the decoded `req.user`), so encrypted-claims /
//! origin-only verification fits. The resolver is codec-agnostic
//! (`Box<dyn Codec>`), so the edge-verifiable asymmetric verifier (cheers
//! R019-F2) drops in later via [`CookieSessionResolver::with_codec`] without
//! touching this file's callers.
//!
//! **`req.user.attrs` is preserved** (R009 decision): cheers `Claims` carries no
//! opaque attribute bag, so the device id, binding, and token lifetimes are
//! folded into `attrs` to keep mesofact's `{ id, attrs }` render contract
//! intact. When cheers grows a first-class extensions field (coordinate with
//! R019), surface it here instead.
//!
//! See `.yah/docs/architecture/mesofact.md` §"Auth & session contract".
//!
//! @yah:ticket(R750-F2, "op_mesofact_session: hand the Rust-resolved User (cheers SessionResolver) into the SSR isolate")
//! @yah:status(review)
//! @yah:at(2026-10-07T21:52:46Z)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R750)
//! @yah:next("Tier: Warrior. Identity is already resolved Rust-side (R556-B13): SessionResolver::resolve(cookie_header) -> Option<User> at oss/mesofact/crates/mesofact-core/src/proxy/session.rs:70, User at :41. The isolate just needs it handed over — mesofact-ssr must NOT gain a cheers dependency.")
//! @yah:next("Design: the SSR dispatch entry in mesofact-ssr (ssr.rs dispatch/invoke path) takes an optional resolved user (serde_json::Value or a small mesofact-ssr-owned SsrUser {id, attrs}) alongside the request, stores it in OpState per-dispatch; #[op2] op_mesofact_session() returns it (or null). The caller in mesofact-core (wherever the SSR route handler builds the request and has the cookie header + resolver) resolves and passes it. If a cookie_header-taking op is strictly required by the ticket text, implement it as op_mesofact_session(cookie_header) that looks up a resolver closure in OpState instead; prefer the pre-resolved shape — fewer moving parts.")
//! @yah:next("JS side: expose it on the runtime API the TS package expects (check oss/mesofact/packages/mesofact-runtime/src for a session/user/auth export; if none exists, add `session()` / `currentUser()` to ssr_harness.js's globalThis.__mesofact_ssr context object and mirror the TS type in mesofact-runtime).")
//! @yah:next("Add a unit test in mesofact-ssr that dispatches a tiny route reading the session and asserts both the user-present and null cases.")
//! @yah:next("Return: single JSON object {ticket_id, status, commit_sha?, notes<=3 sentences with pass/fail counts vs baseline}. Full account goes in @yah:handoff. Git policy is defer: no commits; print the command you would have run.")
//! @yah:verify("cargo test -p mesofact-ssr and cargo test -p mesofact-core pass (record counts vs baseline).")
//! @yah:verify("cargo tree -p mesofact-ssr does not list cheers.")
//! @yah:gotcha("Depends on MFT-R750-F1 landing the extension rewrite in ssr.rs first — register the op in the same extensions() fn. Do not edit js/ssr_runtime_shim.js (R820 header; T3's seam). Shared tree, git policy defer.")
//! @yah:depends_on(MFT-R750-F1)
//! @yah:files(oss/mesofact/crates/mesofact-core/src/proxy/session.rs)
//! @yah:files(oss/mesofact/crates/mesofact-ssr/src/ssr.rs)
//! @yah:files(oss/mesofact/crates/mesofact-ssr/js/ssr_harness.js)
//! @yah:handoff("LANDED: DispatchRequest gains `#[serde(skip)] pub user: Option<serde_json::Value>`, which is mesofact-core's User as {id, attrs}. The isolate's Job::Dispatch puts it in OpState as DispatchSession before dispatch_harness and resets it to None afterwards. An isolate serves one dispatch at a time, so the user cannot leak into the next request. New src/ops_session.rs: sync #[op2] op_mesofact_session() returns the user or null, registered in extensions() next to mesofact_fetch. js/ssr_harness.js: __mesofact_ssr.currentUser(). mesofact-runtime: `SsrContext { currentUser(): User | null }` type in contract.ts, exported from index.ts. ssr_runtime_shim.js was not touched.")
//! @yah:verify("cargo test -p mesofact-ssr: 11 passed, 0 failed (baseline 10, plus the new dispatch_hands_the_resolved_user_to_route_code test, which covers user-present, then null, and no leak across dispatches)")
//! @yah:verify("cargo test -p mesofact-core: 77 passed (48+3+1+21+4), 0 failed; baseline measured before the change was also 77")
//! @yah:verify("cargo check -p mesofact --features ssr --tests: EXIT 0")
//! @yah:verify("cargo tree -p mesofact-ssr | grep -c cheers = 0")
//! @yah:verify("packages/mesofact-runtime tsc --noEmit: exit 0")
//! @yah:handoff("PRODUCTION CALLER WIRED (leader decision). proxy and serve are separate subcommands, i.e. separate processes, so they cannot share one resolver instance. They share the builder instead: the new mesofact_core::proxy::session::resolver_from_env(secret_env, cookie_name) is the body of proxy's old build_session_resolver, and cli/proxy.rs now delegates to it. `mesofact serve` gains the same --session-secret-env / --session-cookie flags with the same env vars (MESOFACT_SESSION_SECRET_ENV / MESOFACT_SESSION_COOKIE). with_session_resolver() attaches the resolver on both SSR paths: the bundle path after attach_bundle_ssr, and run_workload_modes. Server/ServerState hold `session: Option<Arc<dyn SessionResolver>>` (ssr builds; Server::with_session). serve_dynamic calls resolve_ssr_user(resolver, headers), which takes the Cookie header, runs SessionResolver::resolve and serializes the User to {id, attrs}, then passes the result to dispatch_to_ssr. DispatchRequest.user is set on every retry attempt. Not done in this ticket: serve still does not ENFORCE requires:[user]. The 401/redirect gate stays in the proxy router, and --trust-edge-auth is unchanged; the serve_policy_support doc was updated to say so.")
//! @yah:verify("Server-level test ssr_dispatch_carries_the_resolved_user (crates/mesofact/src/server.rs) covers three cases: a verifying cookie gives {id:u_1, attrs:{}}, a missing cookie gives null, and no resolver gives null.")
//! @yah:verify("cargo test -p mesofact --features ssr --no-fail-fast: lib 202 passed, 2 failed (baseline 201 passed, 2 failed). The 2 failures are the same cli::new version-pin tests in both runs (@mesofact/runtime 0.8.42 vs binary 0.8.43-pre.1) and are unrelated to this change. Integration tests: 5 + 2 passed.")
//! @yah:verify("Re-run after the wiring: mesofact-ssr 11 passed; mesofact-core 77 passed; cargo check -p mesofact --tests with and without the ssr feature: EXIT 0. The one warning (resolve_mirror_key never used, default features) is in code this change did not touch.")

use cheers_core::{Claims, Codec, CodecError};
// Concrete symmetric codec moved out of cheers-core into cheers-server by the
// F6 crate split (cheers-core is now the keyless trait/identity surface).
use cheers_server::PasetoV4Codec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_COOKIE_NAME: &str = "mesofact_session";

/// Resolved identity handed to render on `req.user`. `attrs` is opaque to
/// mesofact — populated from the verified cheers [`Claims`] (see
/// [`User::from_claims`]); it rides through to the worker on `req.user`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub attrs: serde_json::Map<String, serde_json::Value>,
}

impl User {
    /// Map verified cheers [`Claims`] onto the render-facing identity. cheers
    /// has no opaque attribute bag, so the device binding + token lifetimes ride
    /// through under `attrs` to preserve mesofact's `{ id, attrs }` render
    /// contract (R009).
    fn from_claims(c: Claims) -> Self {
        let mut attrs = serde_json::Map::new();
        attrs.insert("device".into(), serde_json::Value::String(c.device.into_inner()));
        attrs.insert(
            "binding".into(),
            serde_json::to_value(&c.binding).unwrap_or(serde_json::Value::Null),
        );
        attrs.insert("issued_at".into(), serde_json::json!(c.issued_at));
        attrs.insert("expires_at".into(), serde_json::json!(c.expires_at));
        Self { id: c.sub.into_inner(), attrs }
    }
}

/// Pluggable session resolution. Sync because cookie verification needs no I/O;
/// a network-backed resolver (OAuth introspection) would add its own runtime.
pub trait SessionResolver: Send + Sync {
    /// Resolve identity from a raw `Cookie` header value (or `None` if absent).
    /// Returns `None` for any unauthenticated outcome (missing / bad / expired).
    fn resolve(&self, cookie_header: Option<&str>) -> Option<User>;
}

pub struct CookieSessionResolver {
    cookie_name: String,
    codec: Box<dyn Codec + Send + Sync>,
}

impl CookieSessionResolver {
    /// Build a resolver from a raw secret of any length. The secret is hashed to
    /// a 32-byte key (cheers codecs require exactly 32 bytes) and used to
    /// construct the default [`PasetoV4Codec`]. Pre-launch there are no legacy
    /// tokens, so this key-derivation has no backward-compat path.
    pub fn new(cookie_name: impl Into<String>, secret: impl AsRef<[u8]>) -> Self {
        let codec = PasetoV4Codec::new(&derive_key(secret.as_ref()))
            .expect("a 32-byte key is always valid");
        Self::with_codec(cookie_name, Box::new(codec))
    }

    /// Inject any [`cheers_core::Codec`] — used by tests and forward-looking for
    /// the asymmetric edge verifier (cheers R019). The codec owns the wire
    /// format and crypto; the resolver only does cookie extraction + claim
    /// mapping.
    pub fn with_codec(
        cookie_name: impl Into<String>,
        codec: Box<dyn Codec + Send + Sync>,
    ) -> Self {
        Self { cookie_name: cookie_name.into(), codec }
    }

    /// Mint a token for the given claims — used by tests and any first-party
    /// login endpoint that issues mesofact sessions directly.
    pub fn mint(&self, claims: &Claims) -> Result<String, CodecError> {
        self.codec.mint(claims)
    }
}

impl SessionResolver for CookieSessionResolver {
    fn resolve(&self, cookie_header: Option<&str>) -> Option<User> {
        let token = cookie_value(cookie_header?, &self.cookie_name)?;
        // Codec verifies signature/AEAD *and* rejects expired tokens against the
        // system clock; any failure → unauthenticated.
        let claims = self.codec.verify(token).ok()?;
        Some(User::from_claims(claims))
    }
}

/// Build the cookie resolver from deploy config: `secret_env` names the env var
/// holding the codec secret (`--session-secret-env`), `cookie_name` the cookie.
/// Shared by `mesofact proxy` and `mesofact serve` (R750-F2) so both
/// subcommands resolve sessions from one configuration surface. A
/// configured-but-unset/empty env var is a deploy error: warn and run without
/// sessions rather than crash (fails safe — routes see no user, not a forged one).
pub fn resolver_from_env(
    secret_env: Option<&str>,
    cookie_name: &str,
) -> Option<std::sync::Arc<dyn SessionResolver>> {
    let env_name = secret_env?;
    match std::env::var(env_name) {
        Ok(secret) if !secret.is_empty() => {
            tracing::info!(cookie = %cookie_name, "session resolver enabled");
            Some(std::sync::Arc::new(CookieSessionResolver::new(
                cookie_name.to_owned(),
                secret.into_bytes(),
            )))
        }
        _ => {
            tracing::warn!(
                env = %env_name,
                "session secret env var is unset/empty — sessions disabled"
            );
            None
        }
    }
}

/// Derive a fixed 32-byte codec key from an arbitrary-length deploy secret.
fn derive_key(secret: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(secret);
    h.finalize().into()
}

/// Pull one cookie value out of a `Cookie:` header (`a=1; b=2`). Returns a slice
/// of the header so no allocation happens on the hot path.
fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == name).then(|| v.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheers_core::{DeviceBinding, DeviceId, UserId};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn resolver() -> CookieSessionResolver {
        CookieSessionResolver::new(DEFAULT_COOKIE_NAME, b"super-secret-key")
    }

    fn now() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn claims(user_id: &str, expires_at: i64) -> Claims {
        Claims::new(
            UserId::new(user_id),
            DeviceId::new("d1"),
            DeviceBinding::Passkey,
            now(),
            expires_at,
        )
    }

    #[test]
    fn round_trips_a_signed_session() {
        let r = resolver();
        let token = r.mint(&claims("u42", now() + 3600)).unwrap();
        let user = r.resolve(Some(&format!("mesofact_session={token}"))).unwrap();
        assert_eq!(user.id, "u42");
        // Claims fold into attrs to preserve the `{ id, attrs }` render shape.
        assert_eq!(user.attrs.get("device").unwrap(), &serde_json::json!("d1"));
        assert_eq!(
            user.attrs.get("binding").unwrap(),
            &serde_json::json!({ "kind": "passkey" })
        );
    }

    #[test]
    fn picks_the_named_cookie_out_of_many() {
        let r = resolver();
        let token = r.mint(&claims("u1", now() + 3600)).unwrap();
        let header = format!("theme=dark; mesofact_session={token}; tz=utc");
        assert_eq!(r.resolve(Some(&header)).unwrap().id, "u1");
    }

    #[test]
    fn missing_cookie_resolves_to_none() {
        assert!(resolver().resolve(None).is_none());
        assert!(resolver().resolve(Some("theme=dark")).is_none());
    }

    #[test]
    fn expired_token_resolves_to_none() {
        let r = resolver();
        let token = r.mint(&claims("u1", now() - 1)).unwrap();
        assert!(r.resolve(Some(&format!("mesofact_session={token}"))).is_none());
    }

    #[test]
    fn tampered_token_fails_verification() {
        let r = resolver();
        let token = r.mint(&claims("u1", now() + 3600)).unwrap();
        // Flip a byte in the ciphertext body; AEAD verification must reject it.
        let mut bytes = token.into_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let forged = String::from_utf8(bytes).unwrap();
        assert!(r.resolve(Some(&format!("mesofact_session={forged}"))).is_none());
    }

    #[test]
    fn wrong_key_fails_verification() {
        let signer = resolver();
        let token = signer.mint(&claims("u1", now() + 3600)).unwrap();
        let other = CookieSessionResolver::new(DEFAULT_COOKIE_NAME, b"different-key");
        assert!(other.resolve(Some(&format!("mesofact_session={token}"))).is_none());
    }
}
