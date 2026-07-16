//! S3-compatible [`ObjectStore`] over `reqwest` with hand-rolled SigV4 signing.
//!
//! Mirrors the runtime's `packages/mesofact-runtime/src/adapters/r2.ts` —
//! same path-style URLs, same set of operations (GET/HEAD/PUT/LIST/DELETE),
//! and the same R2 quirks (account-scoped endpoint, region `"auto"`). The
//! TS adapter delegates signing to `aws4fetch`; here we inline the SigV4
//! steps to keep the dep graph tight (mesofact is meant to spin out to its
//! own repo) and match the operation set exactly.
//!
//! Production wiring: instantiate via [`S3Store::new`] from a
//! [`PublishConfig`](crate::config::PublishConfig) +
//! [`S3Credentials`](crate::config::S3Credentials) pair. Real-network
//! exercise is gated on the CI smoke job — local `cargo test` doesn't reach
//! this module (tests use [`InMemoryStore`](crate::object_store::InMemoryStore)).

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, Response};
use sha2::{Digest, Sha256};

use crate::object_store::{ObjectMeta, ObjectStore, PutOpts, StoreError};

const CONTENT_HASH_HEADER: &str = "x-amz-meta-content-hash";

/// Path segments: encode everything but the unreserved set. `/` is preserved
/// by encoding segments individually (matches `encodeKey` in the TS adapter).
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'+')
    .add(b'&')
    .add(b'=')
    .add(b':')
    .add(b';')
    .add(b'@')
    .add(b'!')
    .add(b'$')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b',');

/// SigV4 canonical-query encoding: same set as path segments plus `/`.
const QUERY_VALUE: &AsciiSet = &PATH_SEGMENT.add(b'/');

pub struct S3Store {
    client: Client,
    /// Endpoint root without trailing slash, e.g.
    /// `https://<account>.r2.cloudflarestorage.com`.
    endpoint: String,
    bucket: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
}

impl std::fmt::Debug for S3Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Store")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key_id", &"<redacted>")
            .finish()
    }
}

impl S3Store {
    pub fn new(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Result<Self, StoreError> {
        let client = Client::builder()
            .build()
            .map_err(|e| StoreError::Transport(format!("reqwest build: {e}")))?;
        let endpoint = endpoint.into();
        let endpoint = endpoint.trim_end_matches('/').to_string();
        Ok(Self {
            client,
            endpoint,
            bucket: bucket.into(),
            region: region.into(),
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
        })
    }

    fn key_url(&self, key: &str) -> String {
        let encoded = encode_key(key);
        format!("{}/{}/{}", self.endpoint, self.bucket, encoded)
    }

    fn list_url(&self, prefix: &str, continuation: Option<&str>) -> (String, Vec<(String, String)>) {
        let mut query = vec![
            ("list-type".to_string(), "2".to_string()),
            ("prefix".to_string(), prefix.to_string()),
        ];
        if let Some(token) = continuation {
            query.push(("continuation-token".to_string(), token.to_string()));
        }
        let url = format!("{}/{}", self.endpoint, self.bucket);
        (url, query)
    }
}

#[async_trait]
impl ObjectStore for S3Store {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, StoreError> {
        let res = self
            .signed_request(Method::GET, &self.key_url(key), &[], HeaderMap::new(), Bytes::new())
            .await?;
        if res.status() == 404 {
            return Ok(None);
        }
        let res = check_status(res, "GET", key).await?;
        let bytes = res
            .bytes()
            .await
            .map_err(|e| StoreError::Transport(format!("read body: {e}")))?;
        Ok(Some(bytes))
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>, StoreError> {
        let res = self
            .signed_request(Method::HEAD, &self.key_url(key), &[], HeaderMap::new(), Bytes::new())
            .await?;
        if res.status() == 404 {
            return Ok(None);
        }
        let res = check_status(res, "HEAD", key).await?;
        let headers = res.headers();
        let content_hash = headers
            .get(CONTENT_HASH_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let size = headers
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        Ok(Some(ObjectMeta {
            content_hash,
            content_type,
            size,
        }))
    }

    async fn put(&self, key: &str, body: Bytes, opts: PutOpts) -> Result<(), StoreError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            header_value(&opts.content_type)?,
        );
        if let Some(cc) = &opts.cache_control {
            headers.insert(reqwest::header::CACHE_CONTROL, header_value(cc)?);
        }
        if !opts.content_hash.is_empty() {
            headers.insert(
                HeaderName::from_static(CONTENT_HASH_HEADER),
                header_value(&opts.content_hash)?,
            );
        }
        let res = self
            .signed_request(Method::PUT, &self.key_url(key), &[], headers, body)
            .await?;
        let _ = check_status(res, "PUT", key).await?;
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let mut keys = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let (url, query) = self.list_url(prefix, continuation.as_deref());
            let query_refs: Vec<(&str, &str)> =
                query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let res = self
                .signed_request(Method::GET, &url, &query_refs, HeaderMap::new(), Bytes::new())
                .await?;
            let res = check_status(res, "LIST", prefix).await?;
            let xml = res
                .text()
                .await
                .map_err(|e| StoreError::Transport(format!("read body: {e}")))?;
            let (page_keys, next_token) = parse_list_v2(&xml);
            keys.extend(page_keys);
            if let Some(token) = next_token {
                continuation = Some(token);
            } else {
                break;
            }
        }
        keys.sort();
        Ok(keys)
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        let res = self
            .signed_request(
                Method::DELETE,
                &self.key_url(key),
                &[],
                HeaderMap::new(),
                Bytes::new(),
            )
            .await?;
        // S3 returns 204 on success, 404 for missing — both are no-ops here.
        if res.status() == 404 {
            return Ok(());
        }
        let _ = check_status(res, "DELETE", key).await?;
        Ok(())
    }
}

impl S3Store {
    /// Sign + dispatch one request. `query` is the canonical query list (already
    /// in lookup order); the signer sorts + percent-encodes it.
    async fn signed_request(
        &self,
        method: Method,
        url: &str,
        query: &[(&str, &str)],
        mut headers: HeaderMap,
        body: Bytes,
    ) -> Result<Response, StoreError> {
        let now: DateTime<Utc> = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        let parsed = reqwest::Url::parse(url)
            .map_err(|e| StoreError::Transport(format!("parse url {url}: {e}")))?;
        let host_str = parsed
            .host_str()
            .ok_or_else(|| StoreError::Transport(format!("no host in {url}")))?;
        // SigV4 must sign the exact `Host` header on the wire. reqwest includes
        // an explicit non-default port in `Host` (e.g. a loopback dev endpoint,
        // MinIO, localstack) but omits it for the scheme default (443/80).
        // `Url::port()` returns `Some` only for a non-default port, so it tracks
        // reqwest's behavior — signing bare `host_str()` here would drop the
        // port and 403 (SignatureDoesNotMatch) against any custom-port endpoint.
        let host = match parsed.port() {
            Some(port) => format!("{host_str}:{port}"),
            None => host_str.to_string(),
        };
        let path = if parsed.path().is_empty() {
            "/".to_string()
        } else {
            parsed.path().to_string()
        };

        let payload_hash = sha256_hex(&body);

        // Mandatory signed headers — host, x-amz-content-sha256, x-amz-date —
        // plus any caller-provided headers (content-type, cache-control,
        // x-amz-meta-content-hash on PUT).
        headers.insert(reqwest::header::HOST, header_value(&host)?);
        headers.insert(
            HeaderName::from_static("x-amz-content-sha256"),
            header_value(&payload_hash)?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-date"),
            header_value(&amz_date)?,
        );

        let (canonical_headers, signed_headers) = canonicalize_headers(&headers);
        let canonical_query = canonicalize_query(query);

        let canonical_request = format!(
            "{method}\n{path}\n{query}\n{headers}\n{signed}\n{payload}",
            method = method.as_str(),
            path = path,
            query = canonical_query,
            headers = canonical_headers,
            signed = signed_headers,
            payload = payload_hash,
        );

        let credential_scope = format!("{date_stamp}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{hash}",
            hash = sha256_hex(canonical_request.as_bytes()),
        );

        let signing_key =
            derive_signing_key(&self.secret_access_key, &date_stamp, &self.region, "s3");
        let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={key}/{scope}, SignedHeaders={signed}, Signature={sig}",
            key = self.access_key_id,
            scope = credential_scope,
            signed = signed_headers,
            sig = signature,
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            header_value(&authorization)?,
        );

        // Build the full URL with the SigV4-canonicalized query string already
        // attached so reqwest treats it as opaque — re-encoding through
        // `RequestBuilder::query` risks mismatching the signed canonical form.
        let final_url = if canonical_query.is_empty() {
            url.to_string()
        } else {
            format!("{url}?{canonical_query}")
        };
        // reqwest sets its own Host from the URL; we drop ours from the wire
        // payload but keep it in the signed-headers list (where SigV4 needs it).
        let mut send_headers = headers.clone();
        send_headers.remove(reqwest::header::HOST);
        self.client
            .request(method, &final_url)
            .headers(send_headers)
            .body(body)
            .send()
            .await
            .map_err(|e| StoreError::Transport(format!("{e}")))
    }
}

fn encode_key(key: &str) -> String {
    key.split('/')
        .map(|seg| utf8_percent_encode(seg, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn header_value(s: &str) -> Result<HeaderValue, StoreError> {
    HeaderValue::from_str(s).map_err(|e| StoreError::Transport(format!("invalid header value: {e}")))
}

async fn check_status(res: Response, op: &str, target: &str) -> Result<Response, StoreError> {
    if res.status().is_success() {
        Ok(res)
    } else {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        Err(StoreError::Transport(format!(
            "{op} {target} → HTTP {status}: {body}"
        )))
    }
}

fn sha256_hex(body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(body);
    hex::encode(h.finalize())
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn derive_signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{secret}");
    let k_date = hmac_sha256(k_secret.as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Canonical headers per SigV4: lowercase name, trimmed value, sorted by name,
/// joined with `\n`, terminated by an empty line. `signed_headers` is the
/// `;`-joined list of names.
fn canonicalize_headers(headers: &HeaderMap) -> (String, String) {
    let mut pairs: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_ascii_lowercase(),
                v.to_str().unwrap_or("").trim().to_string(),
            )
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut canonical = String::new();
    let mut names: Vec<&str> = Vec::new();
    for (k, v) in &pairs {
        canonical.push_str(k);
        canonical.push(':');
        canonical.push_str(v);
        canonical.push('\n');
        names.push(k);
    }
    (canonical, names.join(";"))
}

/// Canonical query string per SigV4: keys sorted lexicographically (then by
/// value for repeats), each key+value percent-encoded with the AWS encoding
/// set.
fn canonicalize_query(query: &[(&str, &str)]) -> String {
    let mut entries: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| {
            (
                utf8_percent_encode(k, QUERY_VALUE).to_string(),
                utf8_percent_encode(v, QUERY_VALUE).to_string(),
            )
        })
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Minimal ListBucketResult v2 parser. S3's XML shape is fixed; the contract
/// only exposes object keys, and we want the continuation token for paging.
/// Mirrors `parseListV2` in the TS adapter, plus the truncation handling.
fn parse_list_v2(xml: &str) -> (Vec<String>, Option<String>) {
    let mut keys = Vec::new();
    let mut start = 0;
    while let Some(open) = xml[start..].find("<Contents>") {
        let abs_open = start + open + "<Contents>".len();
        let Some(close_rel) = xml[abs_open..].find("</Contents>") else {
            break;
        };
        let inner = &xml[abs_open..abs_open + close_rel];
        if let Some(key) = pick_xml(inner, "Key") {
            keys.push(key);
        }
        start = abs_open + close_rel + "</Contents>".len();
    }
    let truncated = pick_xml(xml, "IsTruncated")
        .map(|s| s == "true")
        .unwrap_or(false);
    let next_token = if truncated {
        pick_xml(xml, "NextContinuationToken")
    } else {
        None
    };
    (keys, next_token)
}

fn pick_xml(haystack: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let i = haystack.find(&open)? + open.len();
    let j = haystack[i..].find(&close)?;
    Some(haystack[i..i + j].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_key_preserves_path_separators() {
        assert_eq!(encode_key("foo/bar baz.html"), "foo/bar%20baz.html");
        assert_eq!(
            encode_key("build-2026/html/about.html"),
            "build-2026/html/about.html"
        );
    }

    #[test]
    fn canonical_query_sorts_and_encodes() {
        let q = vec![
            ("prefix", "html/"),
            ("list-type", "2"),
            ("continuation-token", "abc/def"),
        ];
        let canon = canonicalize_query(&q);
        // Sorted lexicographically by encoded key. The token's `/` is encoded.
        assert_eq!(
            canon,
            "continuation-token=abc%2Fdef&list-type=2&prefix=html%2F"
        );
    }

    #[test]
    fn signing_key_matches_aws_test_vector() {
        // SigV4 spec test vector for sigv4-signing-key.
        let key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex::encode(key),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn parse_list_v2_picks_keys_and_continuation() {
        let xml = r#"<?xml version="1.0"?><ListBucketResult>
          <IsTruncated>true</IsTruncated>
          <NextContinuationToken>tok123</NextContinuationToken>
          <Contents><Key>a/b.html</Key><Size>10</Size></Contents>
          <Contents><Key>a/c.html</Key><Size>20</Size></Contents>
        </ListBucketResult>"#;
        let (keys, next) = parse_list_v2(xml);
        assert_eq!(keys, vec!["a/b.html".to_string(), "a/c.html".to_string()]);
        assert_eq!(next.as_deref(), Some("tok123"));
    }

    #[test]
    fn parse_list_v2_no_truncation_drops_token() {
        let xml = r#"<ListBucketResult>
          <IsTruncated>false</IsTruncated>
          <NextContinuationToken>stale</NextContinuationToken>
          <Contents><Key>only.html</Key></Contents>
        </ListBucketResult>"#;
        let (keys, next) = parse_list_v2(xml);
        assert_eq!(keys, vec!["only.html"]);
        assert!(next.is_none());
    }
}
