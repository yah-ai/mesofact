//! npm `overrides` / yarn `resolutions` (R773-F6).
//!
//! An override replaces the range a *requester* declared, so it has to reach
//! transitive edges and not just the root's own — which is why the parsed table
//! rides on [`crate::resolve::ResolvedTree`] and is consulted in
//! `Resolver::requirement`, the single funnel both edge sources pass through.
//!
//! # An unsupported form is an error, never a silent drop
//!
//! npm's full grammar is larger than what a flat name → spec table can express:
//! a nested object scopes the override to one dependency path, a `.` key
//! self-references the enclosing scope, a `foo@1.2.3` key only fires when the
//! declared range matches. Accepting one of those and ignoring it would produce
//! a tree that is wrong and *looks right* — the worst outcome on offer here, and
//! one that surfaces as a runtime failure in someone else's project. So every
//! form this module does not implement is refused by name, at parse time,
//! naming the key.

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

use crate::spec::validate_package_name;

/// The root's declared dependency ranges, by name — what a `$name` reference
/// resolves against.
pub(crate) type DeclaredRanges = BTreeMap<String, String>;

/// Parse an `overrides` (npm) or `resolutions` (yarn) object into a flat
/// name → spec table.
///
/// `field` names the source key in every error, because a project carrying both
/// needs to be told which one it got wrong.
pub(crate) fn parse(
    field: &str,
    value: Option<&Value>,
    declared: &DeclaredRanges,
) -> Result<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let Value::Object(entries) = value else {
        bail!("`{field}` must be an object mapping a package name to a version or range");
    };
    let mut out = BTreeMap::new();
    for (key, entry) in entries {
        let name = key_name(field, key)?;
        let spec = entry_spec(field, key, entry, declared)?;
        out.insert(name, spec);
    }
    Ok(out)
}

/// Merge yarn's `resolutions` under npm's `overrides`, which wins on a key
/// both declare — npm's own behaviour when a manifest carries both.
pub(crate) fn merge(
    overrides: BTreeMap<String, String>,
    resolutions: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = resolutions;
    out.extend(overrides);
    out
}

/// The bare package name a key must be, or an error naming the form it is
/// instead.
fn key_name(field: &str, key: &str) -> Result<String> {
    if key == "." {
        bail!(
            "`{field}` key \".\" is npm's nested self-reference form, which this resolver does \
             not support; state the package and its version as a top-level `{field}` entry"
        );
    }
    if key.contains('*') || key.contains('>') || key.contains(' ') {
        bail!(
            "`{field}` key {key:?} is a path-scoped form (yarn's `**/pkg`, npm's `a > b`), which \
             this resolver does not support; only a bare package name is"
        );
    }
    // A scoped name spends its one legal `@` and `/` on the scope; anything
    // after that is a selector or a path, neither of which this table models.
    let rest = match key.strip_prefix('@') {
        Some(after) => after.split_once('/').map_or(after, |(_, rest)| rest),
        None => key,
    };
    if rest.contains('@') {
        bail!(
            "`{field}` key {key:?} carries a version selector, which this resolver does not \
             support: an override here applies to every request for the package, so it cannot be \
             conditioned on the range that was asked for"
        );
    }
    if rest.contains('/') {
        bail!(
            "`{field}` key {key:?} is a dependency path, which this resolver does not support; \
             only a bare package name is"
        );
    }
    validate_package_name(key)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("`{field}` key {key:?}"))?;
    Ok(key.to_string())
}

/// The spec an entry states. A string is taken as-is (after resolving a `$`
/// reference); an object is npm's nested form and is refused.
fn entry_spec(field: &str, key: &str, entry: &Value, declared: &DeclaredRanges) -> Result<String> {
    match entry {
        Value::String(spec) => match spec.strip_prefix('$') {
            Some(reference) => declared.get(reference).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "`{field}.{key}` references `${reference}`, but the root manifest declares no \
                     dependency named {reference:?} to take a version from"
                )
            }),
            None => Ok(spec.clone()),
        },
        Value::Object(nested) => bail!(
            "`{field}.{key}` is npm's nested form ({}), which this resolver does not support: an \
             override here applies throughout the tree rather than only beneath {key:?}. State \
             the inner package as its own top-level `{field}` entry, or pin it in `dependencies`",
            describe_nested(nested)
        ),
        _ => bail!("`{field}.{key}` must be a version or range string"),
    }
}

/// The nested keys, so the error says which packages were being scoped rather
/// than only that something was.
fn describe_nested(nested: &Map<String, Value>) -> String {
    let names: Vec<&str> = nested.keys().map(String::as_str).take(4).collect();
    format!("scoping {}", names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declared() -> DeclaredRanges {
        BTreeMap::from([("react".to_string(), "^18.2.0".to_string())])
    }

    fn parse_ok(field: &str, value: Value) -> BTreeMap<String, String> {
        parse(field, Some(&value), &declared()).expect("parses")
    }

    fn parse_err(field: &str, value: Value) -> String {
        parse(field, Some(&value), &declared()).unwrap_err().to_string()
    }

    #[test]
    fn the_flat_form_parses_including_scoped_names() {
        let table = parse_ok("overrides", json!({ "left-pad": "1.3.0", "@scope/pkg": "^2" }));
        assert_eq!(table["left-pad"], "1.3.0");
        assert_eq!(table["@scope/pkg"], "^2");
    }

    #[test]
    fn a_dollar_reference_takes_the_roots_own_declared_range() {
        let table = parse_ok("overrides", json!({ "react-dom": "$react" }));
        assert_eq!(table["react-dom"], "^18.2.0");

        let err = parse_err("overrides", json!({ "react-dom": "$nope" }));
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn every_unsupported_form_is_refused_by_name() {
        let nested = parse_err("overrides", json!({ "foo": { "bar": "1.0.0" } }));
        assert!(nested.contains("nested form"), "{nested}");
        assert!(nested.contains("bar"), "{nested}");

        let selfref = parse_err("overrides", json!({ ".": "1.0.0" }));
        assert!(selfref.contains("self-reference"), "{selfref}");

        let selector = parse_err("overrides", json!({ "foo@1.2.3": "1.2.4" }));
        assert!(selector.contains("version selector"), "{selector}");

        let glob = parse_err("resolutions", json!({ "**/foo": "1.0.0" }));
        assert!(glob.contains("path-scoped"), "{glob}");
        assert!(glob.contains("resolutions"), "{glob}");

        let path = parse_err("resolutions", json!({ "foo/bar": "1.0.0" }));
        assert!(path.contains("dependency path"), "{path}");

        let not_a_string = parse_err("overrides", json!({ "foo": 3 }));
        assert!(not_a_string.contains("version or range"), "{not_a_string}");

        let not_an_object = parse_err("overrides", json!(["foo"]));
        assert!(not_an_object.contains("must be an object"), "{not_an_object}");
    }

    #[test]
    fn overrides_win_over_resolutions_on_a_shared_key() {
        let merged = merge(
            BTreeMap::from([("a".to_string(), "1".to_string())]),
            BTreeMap::from([("a".to_string(), "2".to_string()), ("b".to_string(), "3".to_string())]),
        );
        assert_eq!(merged["a"], "1");
        assert_eq!(merged["b"], "3");
    }

    #[test]
    fn an_absent_field_is_an_empty_table_not_an_error() {
        assert!(parse("overrides", None, &declared()).expect("absent").is_empty());
    }
}
