//! Source-inference pass — port of
//! `packages/mesofact-build/src/source-infer.ts`. Regex-equivalent scan of
//! the *source* file (not the bundle) for adapter factory calls
//! (`r2('name')`, `sqlite('name')`, `pg('name')`, `rpc('name')`) plus the
//! `// @mesofact-sources a, b` override directive.

use std::collections::BTreeSet;

const ADAPTER_NAMES: &[&str] = &["r2", "sqlite", "pg", "rpc"];

pub struct InferenceResult {
    pub source_reads: Vec<String>,
    pub override_used: bool,
}

pub fn infer_from_source(src: &str) -> InferenceResult {
    let mut overrides: Vec<String> = Vec::new();
    for line_match in find_override_directives(src) {
        for name in line_match.split(',') {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                overrides.push(trimmed.to_string());
            }
        }
    }
    if !overrides.is_empty() {
        return InferenceResult { source_reads: dedupe_sorted(overrides), override_used: true };
    }

    let mut names = Vec::new();
    let bytes = src.as_bytes();
    for adapter in ADAPTER_NAMES {
        let mut start = 0;
        while let Some(pos) = src[start..].find(adapter) {
            let abs = start + pos;
            start = abs + adapter.len();
            // word boundary before
            if abs > 0 {
                let prev = bytes[abs - 1] as char;
                if prev.is_ascii_alphanumeric() || prev == '_' || prev == '$' {
                    continue;
                }
            }
            // `(\s*` then a quote
            let rest = &src[abs + adapter.len()..];
            let after_paren = match rest.trim_start().strip_prefix('(') {
                Some(r) => r.trim_start(),
                None => continue,
            };
            let Some(quote) = after_paren.chars().next().filter(|c| matches!(c, '\'' | '"' | '`'))
            else {
                continue;
            };
            let inner = &after_paren[1..];
            let Some(end) = inner.find(quote) else { continue };
            let name = &inner[..end];
            // Reject names spanning other quote kinds (regex used [^'"`]+)
            if name.contains(['\'', '"', '`']) || name.is_empty() {
                continue;
            }
            // closing paren after optional whitespace
            let after_name = inner[end + 1..].trim_start();
            if !after_name.starts_with(')') {
                continue;
            }
            names.push(name.to_string());
        }
    }
    InferenceResult { source_reads: dedupe_sorted(names), override_used: false }
}

pub fn infer_from_file(path: &std::path::Path) -> anyhow::Result<InferenceResult> {
    let src = std::fs::read_to_string(path)?;
    Ok(infer_from_source(&src))
}

fn find_override_directives(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in src.lines() {
        if let Some(idx) = line.find("//") {
            let comment = line[idx + 2..].trim_start();
            if let Some(rest) = comment.strip_prefix("@mesofact-sources") {
                out.push(rest.trim());
            }
        }
    }
    out
}

fn dedupe_sorted(names: Vec<String>) -> Vec<String> {
    names.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_adapter_calls() {
        let src = r#"
            const a = r2('assets');
            const b = sqlite("db");
            const c = myR2('not-this');
            const d = pg( `proj` );
        "#;
        let r = infer_from_source(src);
        assert_eq!(r.source_reads, vec!["assets", "db", "proj"]);
        assert!(!r.override_used);
    }

    #[test]
    fn override_directive_wins() {
        let src = "// @mesofact-sources foo, bar\nconst a = r2('assets');";
        let r = infer_from_source(src);
        assert_eq!(r.source_reads, vec!["bar", "foo"]);
        assert!(r.override_used);
    }
}
