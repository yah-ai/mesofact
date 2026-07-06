//! Manifest-derived sitemap emitter (W270 §4). The SSG driver collects the
//! URL of every enumerable static-route emission that is not `noindex`;
//! instance-addressed (deferred) routes prerender nothing and so contribute
//! no URLs at all — unlisted-by-capability means no sitemap participation.
//! This module joins those paths onto the configured `site_url` origin and
//! renders a sitemaps.org 0.9 `urlset`.

/// Build a `sitemap.xml` body. `site_url` is the origin (scheme + host, e.g.
/// `https://yah.dev`); each entry in `paths` is a root-relative route path
/// (e.g. `/releases`, `/p/42`). Paths are de-duplicated and sorted so the
/// emitted bytes are reproducible across builds.
pub fn build_sitemap(site_url: &str, paths: &[String]) -> String {
    let base = site_url.trim_end_matches('/');

    let mut locs: Vec<String> = paths
        .iter()
        .map(|p| {
            let path = if p.starts_with('/') { p.clone() } else { format!("/{p}") };
            format!("{base}{path}")
        })
        .collect();
    locs.sort();
    locs.dedup();

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for loc in &locs {
        out.push_str("  <url><loc>");
        xml_escape_into(&mut out, loc);
        out.push_str("</loc></url>\n");
    }
    out.push_str("</urlset>\n");
    out
}

/// XML text escaping for a `<loc>` value. URLs rarely carry these, but a `&`
/// in a query string (or a stray angle bracket from a bad param) must not
/// break the document.
fn xml_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_origin_and_paths_sorted_and_deduped() {
        let xml = build_sitemap(
            "https://yah.dev/",
            &["/releases".into(), "/about".into(), "/releases".into()],
        );
        assert!(xml.contains("<loc>https://yah.dev/about</loc>"));
        assert!(xml.contains("<loc>https://yah.dev/releases</loc>"));
        // deduped: exactly one /releases
        assert_eq!(xml.matches("/releases</loc>").count(), 1);
        // sorted: /about before /releases
        assert!(xml.find("/about").unwrap() < xml.find("/releases").unwrap());
    }

    #[test]
    fn escapes_xml_metacharacters_in_loc() {
        let xml = build_sitemap("https://x.io", &["/q?a=1&b=2".into()]);
        assert!(xml.contains("<loc>https://x.io/q?a=1&amp;b=2</loc>"));
        assert!(!xml.contains("a=1&b=2"));
    }

    #[test]
    fn empty_paths_yields_valid_empty_urlset() {
        let xml = build_sitemap("https://x.io", &[]);
        assert!(xml.contains("<urlset"));
        assert!(xml.contains("</urlset>"));
        assert!(!xml.contains("<url>"));
    }

    #[test]
    fn normalizes_missing_leading_slash() {
        let xml = build_sitemap("https://x.io", &["foo".into()]);
        assert!(xml.contains("<loc>https://x.io/foo</loc>"));
    }
}
