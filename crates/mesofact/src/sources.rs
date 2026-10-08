//! The production [`SourceBackend`] behind SSR's `r2(name)` / `sqlite(name)`
//! (R750-F3). Built once per `ssr::spawn` from the workload's
//! `mesofact.config.toml` `[sources.*]` (resolved by `ssr::resolve_sources`),
//! then attached to every dispatch by `SsrChild`.
//!
//! - `kind = "r2"` → mesofact-publisher's SigV4-signed [`S3Store`] (region
//!   `auto`, as R2 expects). This is what retires the unsigned-request gap the
//!   R820 annotation on `ssr_runtime_shim.js` describes: the isolate no longer
//!   talks to R2 itself.
//! - `kind = "sqlite"` → turso 0.7.2 (operator decision, R750-F3), one
//!   `Database` per source opened lazily on first read, a fresh connection per
//!   call.
//!
//! All I/O runs on the runtime captured at construction (the server's), not on
//! the calling isolate's current-thread runtime: an isolate's runtime is only
//! polled while that isolate is mid-dispatch, so a pooled HTTP connection
//! opened there would stall the next isolate that reused it.

use std::collections::HashMap;
use std::sync::Arc;

use mesofact_publisher::{ObjectStore, S3Store, StoreError};
use mesofact_ssr::{ListOpts, R2Object, SourceBackend, SourceError, SourceFuture};
use serde_json::{Map, Value};
use tokio::sync::OnceCell;

/// A resolved `[sources.<name>]` entry.
#[derive(Debug, Clone)]
pub enum SourceSpec {
    R2 {
        bucket: String,
        endpoint: String,
        access_key_id: String,
        secret_access_key: String,
    },
    Sqlite { path: String },
}

struct Inner {
    r2: HashMap<String, S3Store>,
    sqlite: HashMap<String, (String, OnceCell<turso::Database>)>,
}

pub struct Sources {
    inner: Arc<Inner>,
    rt: tokio::runtime::Handle,
}

impl Sources {
    /// Build from resolved specs. Must be called inside a tokio runtime — that
    /// runtime is where every source read executes.
    pub fn new(specs: Vec<(String, SourceSpec)>) -> anyhow::Result<Self> {
        let mut r2 = HashMap::new();
        let mut sqlite = HashMap::new();
        for (name, spec) in specs {
            match spec {
                SourceSpec::R2 { bucket, endpoint, access_key_id, secret_access_key } => {
                    let store =
                        S3Store::new(endpoint, bucket, "auto", access_key_id, secret_access_key)
                            .map_err(|e| anyhow::anyhow!("[sources.{name}] r2 client: {e}"))?;
                    r2.insert(name, store);
                }
                SourceSpec::Sqlite { path } => {
                    sqlite.insert(name, (path, OnceCell::new()));
                }
            }
        }
        Ok(Self {
            inner: Arc::new(Inner { r2, sqlite }),
            rt: tokio::runtime::Handle::current(),
        })
    }

    /// Run `f` on the captured runtime and await it from wherever we are.
    fn on_rt<'a, T, F, Fut>(&'a self, f: F) -> SourceFuture<'a, T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Inner>) -> Fut,
        Fut: std::future::Future<Output = Result<T, SourceError>> + Send + 'static,
    {
        let task = self.rt.spawn(f(Arc::clone(&self.inner)));
        Box::pin(async move {
            task.await
                .map_err(|e| SourceError::Unavailable(format!("source task failed: {e}")))?
        })
    }
}

fn store_err(e: StoreError) -> SourceError {
    match e {
        StoreError::Transport(m) => SourceError::Unavailable(m),
        other => SourceError::Query(other.to_string()),
    }
}

impl Inner {
    async fn db(&self, name: &str) -> Result<turso::Connection, SourceError> {
        let (path, cell) = self.sqlite.get(name).ok_or(SourceError::NotRegistered)?;
        let db = cell
            .get_or_try_init(|| async { turso::Builder::new_local(path).build().await })
            .await
            .map_err(|e| SourceError::Unavailable(format!("opening {path}: {e}")))?;
        db.connect().map_err(|e| SourceError::Unavailable(format!("connecting {path}: {e}")))
    }

    async fn rows(
        &self,
        name: &str,
        sql: &str,
        params: Vec<turso::Value>,
    ) -> Result<Vec<Value>, SourceError> {
        let conn = self.db(name).await?;
        let mut rows = conn
            .query(sql, turso::params_from_iter(params))
            .await
            .map_err(|e| SourceError::Query(format!("{sql}: {e}")))?;
        let cols = rows.column_names();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| SourceError::Query(e.to_string()))? {
            let mut obj = Map::new();
            for (i, col) in cols.iter().enumerate() {
                let v = row.get_value(i).map_err(|e| SourceError::Query(e.to_string()))?;
                obj.insert(col.clone(), to_json(v));
            }
            out.push(Value::Object(obj));
        }
        Ok(out)
    }
}

fn to_json(v: turso::Value) -> Value {
    match v {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(i) => Value::from(i),
        turso::Value::Real(f) => Value::from(f),
        turso::Value::Text(s) => Value::String(s),
        // bun:sqlite hands blobs back as Uint8Array; JSON has no bytes, so an
        // array of octets is the closest faithful shape.
        turso::Value::Blob(b) => Value::from(b),
    }
}

fn from_json(v: Value) -> Result<turso::Value, SourceError> {
    Ok(match v {
        Value::Null => turso::Value::Null,
        Value::Bool(b) => turso::Value::Integer(b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(i) => turso::Value::Integer(i),
            None => turso::Value::Real(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => turso::Value::Text(s),
        other => {
            return Err(SourceError::Query(format!(
                "unsupported sqlite parameter {other} (null, boolean, number or string only)"
            )))
        }
    })
}

/// Same identifier quoting as packages/mesofact-runtime/src/adapters/sqlite.ts.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

impl SourceBackend for Sources {
    fn fetch<'a>(&'a self, source: &'a str, key: &'a str) -> SourceFuture<'a, Option<Vec<u8>>> {
        let (source, key) = (source.to_owned(), key.to_owned());
        self.on_rt(move |inner| async move {
            let store = inner.r2.get(&source).ok_or(SourceError::NotRegistered)?;
            Ok(store.get(&key).await.map_err(store_err)?.map(|b| b.to_vec()))
        })
    }

    fn list<'a>(
        &'a self,
        source: &'a str,
        prefix: &'a str,
        opts: ListOpts,
    ) -> SourceFuture<'a, Vec<R2Object>> {
        let (source, prefix) = (source.to_owned(), prefix.to_owned());
        self.on_rt(move |inner| async move {
            let store = inner.r2.get(&source).ok_or(SourceError::NotRegistered)?;
            let page = store
                .list_page(&prefix, opts.limit, opts.cursor.as_deref(), opts.delimiter.as_deref())
                .await
                .map_err(store_err)?;
            Ok(page
                .into_iter()
                .map(|o| R2Object {
                    key: o.key,
                    size: o.size,
                    last_modified: o.last_modified,
                    etag: o.etag,
                })
                .collect())
        })
    }

    fn get<'a>(
        &'a self,
        source: &'a str,
        table: &'a str,
        id: &'a str,
    ) -> SourceFuture<'a, Option<Value>> {
        let sql = format!("SELECT * FROM {} WHERE id = ? LIMIT 1", quote_ident(table));
        let (source, id) = (source.to_owned(), id.to_owned());
        self.on_rt(move |inner| async move {
            Ok(inner.rows(&source, &sql, vec![turso::Value::Text(id)]).await?.into_iter().next())
        })
    }

    fn query<'a>(
        &'a self,
        source: &'a str,
        sql: &'a str,
        params: Vec<Value>,
    ) -> SourceFuture<'a, Vec<Value>> {
        let (source, sql) = (source.to_owned(), sql.to_owned());
        self.on_rt(move |inner| async move {
            let params = params.into_iter().map(from_json).collect::<Result<Vec<_>, _>>()?;
            inner.rows(&source, &sql, params).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real turso path end to end: a file-backed db, `get` by id, `query`
    /// with params, a SQL error as `Query`, an unknown name as `NotRegistered`.
    #[tokio::test(flavor = "multi_thread")]
    async fn sqlite_get_and_query_through_turso() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite").to_string_lossy().into_owned();
        {
            let db = turso::Builder::new_local(&path).build().await.unwrap();
            let conn = db.connect().unwrap();
            conn.execute("CREATE TABLE issues (id TEXT PRIMARY KEY, title TEXT, n INTEGER)", ())
                .await
                .unwrap();
            conn.execute("INSERT INTO issues VALUES ('1', 'first', 10), ('2', 'second', 20)", ())
                .await
                .unwrap();
        }
        let src = Sources::new(vec![("db".into(), SourceSpec::Sqlite { path })]).unwrap();

        let row = src.get("db", "issues", "2").await.unwrap();
        assert_eq!(row, Some(serde_json::json!({ "id": "2", "title": "second", "n": 20 })));
        assert_eq!(src.get("db", "issues", "9").await.unwrap(), None);
        let rows = src
            .query("db", "SELECT title FROM issues WHERE n > ? ORDER BY id", vec![serde_json::json!(5)])
            .await
            .unwrap();
        assert_eq!(rows, vec![serde_json::json!({ "title": "first" }), serde_json::json!({ "title": "second" })]);
        assert!(matches!(src.query("db", "SELECT * FROM nope", vec![]).await, Err(SourceError::Query(_))));
        assert_eq!(src.get("other", "issues", "1").await, Err(SourceError::NotRegistered));
        assert_eq!(src.fetch("db", "k").await, Err(SourceError::NotRegistered));
    }
}
