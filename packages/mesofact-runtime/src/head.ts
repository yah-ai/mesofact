// Typed <head> contract (W270 §4). A render MAY return a `head` value on its
// RenderResult; the prerenderer / SSG dispatch weaves it into the document
// head at the same seam as the hydration tags. Consumers return structured
// data — they never hand-assemble head markup or escape it themselves.
//
// Escaping lives HERE, one audited implementation, same posture as
// `escapeJsonForScriptTag` (hydration.ts): all interpolated values are HTML-
// escaped, attribute values additionally escape the quote that closes the
// attribute. Meta keys (`og:title`, `twitter:card`, …) are framework-owned
// literals and are never interpolated from consumer data.
//
// Also ported byte-for-byte into the deno_core SSG runtime shim at
// `crates/mesofact-ssr/js/runtime_shim.js` — keep the two in lockstep so both
// pipelines emit the same head bytes (same rule as the hydration helpers).

export type OpenGraph = {
  title?: string;
  description?: string;
  // og:type — e.g. "website" | "article". Framework passes it through as-is
  // (value is escaped like any other content).
  type?: string;
  url?: string;
  image?: string;
  siteName?: string;
};

export type TwitterCard = {
  // twitter:card — e.g. "summary" | "summary_large_image".
  card?: string;
  title?: string;
  description?: string;
  image?: string;
  site?: string;
  creator?: string;
};

// A generic <link>. `rel` + `href` are the only universally-required
// attributes; both are attribute-escaped. Richer link attrs (sizes, type)
// can be folded in later without breaking this shape.
export type HeadLink = {
  rel: string;
  href: string;
};

export type Head = {
  title?: string;
  description?: string;
  // Emitted as <link rel="canonical" href="…">.
  canonical?: string;
  og?: OpenGraph;
  twitter?: TwitterCard;
  // When true, emits <meta name="robots" content="noindex">. Instance-
  // addressed (deferred) pages set this; enumerable static routes that set it
  // are also dropped from the manifest-derived sitemap.
  noindex?: boolean;
  links?: readonly HeadLink[];
};

// Escape a value destined for HTML text content (`<title>…`): the three
// characters that can open a tag / entity. `&` first so we never double-encode
// the escapes we introduce.
function escapeHtmlText(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Escape a value destined for a double-quoted attribute: text escaping plus
// the closing quote.
function escapeHtmlAttr(value: string): string {
  return escapeHtmlText(value).replace(/"/g, "&quot;");
}

// key is a framework-owned literal (og:title, twitter:card, …); only content
// is consumer-supplied, so only content is escaped.
function metaTag(attr: "name" | "property", key: string, content: string): string {
  return `<meta ${attr}="${key}" content="${escapeHtmlAttr(content)}">`;
}

// Render a Head into the concatenated head-tag markup (no wrapping <head>).
// Order is stable and deterministic so prerendered bytes are reproducible.
export function renderHead(head: Head): string {
  const tags: string[] = [];

  if (head.title !== undefined) tags.push(`<title>${escapeHtmlText(head.title)}</title>`);
  if (head.description !== undefined) tags.push(metaTag("name", "description", head.description));
  if (head.canonical !== undefined) {
    tags.push(`<link rel="canonical" href="${escapeHtmlAttr(head.canonical)}">`);
  }
  if (head.noindex) tags.push(`<meta name="robots" content="noindex">`);

  const og = head.og;
  if (og) {
    if (og.title !== undefined) tags.push(metaTag("property", "og:title", og.title));
    if (og.description !== undefined) {
      tags.push(metaTag("property", "og:description", og.description));
    }
    if (og.type !== undefined) tags.push(metaTag("property", "og:type", og.type));
    if (og.url !== undefined) tags.push(metaTag("property", "og:url", og.url));
    if (og.image !== undefined) tags.push(metaTag("property", "og:image", og.image));
    if (og.siteName !== undefined) tags.push(metaTag("property", "og:site_name", og.siteName));
  }

  const tw = head.twitter;
  if (tw) {
    if (tw.card !== undefined) tags.push(metaTag("name", "twitter:card", tw.card));
    if (tw.title !== undefined) tags.push(metaTag("name", "twitter:title", tw.title));
    if (tw.description !== undefined) {
      tags.push(metaTag("name", "twitter:description", tw.description));
    }
    if (tw.image !== undefined) tags.push(metaTag("name", "twitter:image", tw.image));
    if (tw.site !== undefined) tags.push(metaTag("name", "twitter:site", tw.site));
    if (tw.creator !== undefined) tags.push(metaTag("name", "twitter:creator", tw.creator));
  }

  for (const link of head.links ?? []) {
    tags.push(`<link rel="${escapeHtmlAttr(link.rel)}" href="${escapeHtmlAttr(link.href)}">`);
  }

  return tags.join("");
}

// Weave a Head into a rendered document: inject the head markup immediately
// before the last </head> (case-insensitive). A document without one gets the
// markup prepended. A head that renders to nothing leaves the html untouched.
export function weaveHead(html: string, head: Head): string {
  const markup = renderHead(head);
  if (markup === "") return html;
  const idx = html.toLowerCase().lastIndexOf("</head>");
  if (idx === -1) return markup + html;
  return html.slice(0, idx) + markup + html.slice(idx);
}
