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
