//! Prerender param expansion + data_inputs reads. Port of the
//! `expandPrerenderParams` / `expandFromData` / `walkDottedPath` /
//! `expandRoute` family in `packages/mesofact-build/src/index.ts` and
//! `prerender.ts`.

use anyhow::{bail, Context, Result};
use mesofact_core::manifest::Prerender;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

use crate::route_config::RouteEntry;

/// Expand a route's `prerender` config into concrete param maps.
/// The source-derived shape (`{from, query, param}` walking a registered R2
/// adapter) is not supported in the Rust-native pipeline — use
/// `prerender.from_data` for those routes (see W174 amendment).
pub fn expand_prerender_params(
    r: &RouteEntry,
    project_root: &Path,
) -> Result<Vec<BTreeMap<String, String>>> {
    match &r.prerender {
        // Instance-addressed route: params are minted after the build, so
        // the build prerenders zero instances (render-only entrypoint owns
        // instance production).
        Some(Prerender::Deferred { .. }) => Ok(vec![]),
        other => expand_prerender(&r.route, other.as_ref(), project_root),
    }
}

/// Prerender expansion shared by the build (via [`expand_prerender_params`])
/// and the render-only entrypoint's all-instances form (which re-expands
/// *fresh* at revalidate time). Deferred is a caller decision — the build
/// maps it to zero instances, the render verb rejects it (instances are
/// publish-minted, not enumerable).
pub fn expand_prerender(
    route: &str,
    prerender: Option<&Prerender>,
    project_root: &Path,
) -> Result<Vec<BTreeMap<String, String>>> {
    match prerender {
        None => Ok(vec![BTreeMap::new()]),
        Some(Prerender::Literal { params }) => Ok(params.clone()),
        Some(Prerender::FromData { from_data, items_key, param }) => {
            expand_from_data(route, from_data, items_key, param, project_root)
        }
        Some(Prerender::SourceDerived { from, .. }) => bail!(
            "route {route}: prerender.from='{from}' (source-derived enumeration) is not supported by the Rust-native pipeline; use prerender.from_data instead"
        ),
        Some(Prerender::Deferred { .. }) => bail!(
            "route {route}: prerender.deferred instances are minted at publish time and cannot be enumerated — render them one at a time with explicit params"
        ),
    }
}

fn expand_from_data(
    route: &str,
    from_data: &str,
    items_key: &str,
    param: &str,
    project_root: &Path,
) -> Result<Vec<BTreeMap<String, String>>> {
    let abs = project_root.join(from_data);
    let raw = std::fs::read_to_string(&abs).with_context(|| {
        format!("route {route}: failed reading prerender.from_data='{from_data}'")
    })?;
    let parsed: Value = serde_json::from_str(&raw).with_context(|| {
        format!("route {route}: failed parsing prerender.from_data='{from_data}'")
    })?;
    let Some(items) = walk_dotted_path(&parsed, items_key) else {
        bail!("route {route}: prerender.items_key='{items_key}' not found in {from_data}");
    };
    let Value::Array(items) = items else {
        bail!("route {route}: prerender.items_key='{items_key}' in {from_data} is not an array");
    };
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let Value::Object(obj) = item else {
                bail!("route {route}: prerender.from_data='{from_data}' items[{i}] is not an object");
            };
            let Some(Value::String(value)) = obj.get(param) else {
                let got = obj.get(param).map_or("undefined", value_kind);
                bail!(
                    "route {route}: prerender.from_data='{from_data}' items[{i}].{param} is not a string (got {got})"
                );
            };
            let mut map = BTreeMap::new();
            map.insert(param.to_string(), value.clone());
            Ok(map)
        })
        .collect()
}

fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `"items"` → obj.items, `"data.list"` → obj.data.list,
/// `"rows.0.children"` → obj.rows[0].children.
pub fn walk_dotted_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = root;
    for segment in path.split('.') {
        match cur {
            Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                cur = arr.get(idx)?;
            }
            Value::Object(obj) => {
                cur = obj.get(segment)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Read each declared `data_inputs` file as JSON, keyed by the declared
/// relative path — the `req.data` map handed to render().
pub fn read_data_inputs(
    data_inputs: &[String],
    project_root: &Path,
) -> Result<serde_json::Map<String, Value>> {
    let mut out = serde_json::Map::new();
    for rel in data_inputs {
        let abs = project_root.join(rel);
        let raw = std::fs::read_to_string(&abs)
            .with_context(|| format!("failed reading data_inputs file {}", abs.display()))?;
        let parsed: Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed parsing data_inputs file {}", abs.display()))?;
        out.insert(rel.clone(), parsed);
    }
    Ok(out)
}

/// Substitute `:param` segments with their (URI-encoded) values — port of
/// prerender.ts's `expandRoute`. No params → the route pattern itself.
pub fn expand_route(route: &str, params: &BTreeMap<String, String>) -> Result<String> {
    if params.is_empty() {
        return Ok(route.to_string());
    }
    let mut out = String::with_capacity(route.len());
    let mut rest = route;
    while let Some(pos) = rest.find(':') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos + 1..];
        let end = rest
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
            .map_or(rest.len(), |(i, _)| i);
        let key = &rest[..end];
        let Some(value) = params.get(key) else {
            bail!("route {route}: missing param '{key}' in prerender map");
        };
        out.push_str(&encode_uri_component(value));
        rest = &rest[end..];
    }
    out.push_str(rest);
    Ok(out)
}

// encodeURIComponent parity: unreserved = A-Z a-z 0-9 - _ . ! ~ * ' ( )
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')') {
            out.push(c);
        } else {
            for b in c.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dotted_path_walks_objects_and_arrays() {
        let v = json!({"rows": [{"children": [1, 2]}]});
        assert_eq!(walk_dotted_path(&v, "rows.0.children"), Some(&json!([1, 2])));
        assert_eq!(walk_dotted_path(&v, "rows.x"), None);
        assert_eq!(walk_dotted_path(&v, "missing"), None);
    }

    #[test]
    fn expand_route_encodes_params() {
        let mut p = BTreeMap::new();
        p.insert("id".to_string(), "a/b c".to_string());
        assert_eq!(expand_route("/p/:id", &p).unwrap(), "/p/a%2Fb%20c");
        assert_eq!(expand_route("/", &BTreeMap::new()).unwrap(), "/");
    }
}
