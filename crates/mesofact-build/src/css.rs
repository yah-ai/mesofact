//! LightningCSS step (W174 pillar 3 / R448-F3). The current dogfood apps
//! style via inline TS tokens, so no CSS flows through the JS bundle graph
//! yet — this step is the pipeline slot: standalone stylesheet processing
//! (nesting, custom-properties, prefixing, minification) for `.css` files a
//! workload opts into via `src/styles/*.css` or route-adjacent imports.
//! public/ CSS is copied verbatim by the asset step (serve-exactly-this
//! contract), not minified here.

use anyhow::{anyhow, Result};
use lightningcss::printer::PrinterOptions;
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::targets::{Browsers, Targets};

/// Compile one stylesheet: parse (with nesting + custom-media syntax),
/// lower for the default browser matrix, minify.
pub fn compile_css(source: &str, filename: &str) -> Result<String> {
    let mut sheet = StyleSheet::parse(
        source,
        ParserOptions { filename: filename.to_string(), ..Default::default() },
    )
    .map_err(|e| anyhow!("{filename}: CSS parse failed: {e}"))?;

    let targets = Targets::from(Browsers {
        // Roughly "last 2 years of evergreen" — the same posture the apps'
        // JS transform takes (ES2022). Tighten when a consumer needs it.
        chrome: Some(110 << 16),
        firefox: Some(110 << 16),
        safari: Some(16 << 16),
        edge: Some(110 << 16),
        ..Default::default()
    });

    sheet
        .minify(lightningcss::stylesheet::MinifyOptions { targets, ..Default::default() })
        .map_err(|e| anyhow!("{filename}: CSS minify failed: {e}"))?;

    let out = sheet
        .to_css(PrinterOptions { minify: true, targets, ..Default::default() })
        .map_err(|e| anyhow!("{filename}: CSS print failed: {e}"))?;
    Ok(out.code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_nesting_and_custom_properties() {
        let src = r#"
            :root { --brand: #abcdef; }
            .card {
              color: var(--brand);
              & .title { font-weight: 700; }
            }
        "#;
        let out = compile_css(src, "test.css").unwrap();
        assert!(out.contains("--brand"), "kept custom property: {out}");
        assert!(out.contains(".card .title"), "lowered nesting: {out}");
        assert!(!out.contains('\n') || out.len() < src.len(), "minified: {out}");
    }

    #[test]
    fn rejects_broken_css() {
        // A truncated declaration is *recovered* per the CSS syntax spec
        // (EOF closes open blocks), so use a genuinely invalid selector.
        assert!(compile_css("12px { color: red }", "broken.css").is_err());
    }
}
