// Typed <head> contract — tag shapes, escaping, and the </head> weave (W270 §4).

import { describe, expect, test } from "bun:test";
import { type Head, renderHead, weaveHead } from "../src/index.js";

describe("renderHead", () => {
  test("emits title, description, canonical in a stable order", () => {
    const html = renderHead({
      title: "Releases",
      description: "What shipped",
      canonical: "https://yah.dev/releases",
    });
    expect(html).toBe(
      "<title>Releases</title>" +
        '<meta name="description" content="What shipped">' +
        '<link rel="canonical" href="https://yah.dev/releases">',
    );
  });

  test("emits noindex robots meta only when set", () => {
    expect(renderHead({ noindex: true })).toBe('<meta name="robots" content="noindex">');
    expect(renderHead({ noindex: false })).toBe("");
    expect(renderHead({})).toBe("");
  });

  test("emits og:* with property= and twitter:* with name=", () => {
    const html = renderHead({
      og: { title: "T", description: "D", type: "article", url: "u", image: "i", siteName: "S" },
      twitter: { card: "summary_large_image", title: "TT", image: "ti", site: "@yah" },
    });
    expect(html).toContain('<meta property="og:title" content="T">');
    expect(html).toContain('<meta property="og:site_name" content="S">');
    expect(html).toContain('<meta name="twitter:card" content="summary_large_image">');
    expect(html).toContain('<meta name="twitter:site" content="@yah">');
  });

  test("escapes hostile content in text, attributes, and links", () => {
    const html = renderHead({
      title: 'A & B <script>alert(1)</script>',
      description: 'has "quotes" and <angles>',
      og: { title: '</title><img onerror=x>' },
      links: [{ rel: 'stylesheet"><svg', href: "/a.css?x=1&y=2" }],
    });
    // No raw tag-opening or attribute-breaking characters survive.
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("</title><img");
    expect(html).not.toContain('"quotes"');
    expect(html).not.toContain('stylesheet"><svg"'); // rel attr can't break out
    expect(html).toContain("&amp;");
    expect(html).toContain("&lt;");
    expect(html).toContain("&quot;");
    // & escaped exactly once (not double-encoded).
    expect(html).toContain("/a.css?x=1&amp;y=2");
    expect(html).not.toContain("&amp;amp;");
  });

  test("emits generic links after everything else", () => {
    const html = renderHead({
      links: [
        { rel: "icon", href: "/favicon.ico" },
        { rel: "alternate", href: "/feed.xml" },
      ],
    });
    expect(html).toBe(
      '<link rel="icon" href="/favicon.ico">' + '<link rel="alternate" href="/feed.xml">',
    );
  });
});

describe("weaveHead", () => {
  const head: Head = { title: "Hi" };

  test("injects before the last </head> (case-insensitive)", () => {
    const out = weaveHead("<html><HEAD><meta charset=utf-8></HEAD><body></body></html>", head);
    expect(out).toBe(
      "<html><HEAD><meta charset=utf-8><title>Hi</title></HEAD><body></body></html>",
    );
  });

  test("prepends when there is no </head>", () => {
    expect(weaveHead("<div>no head</div>", head)).toBe("<title>Hi</title><div>no head</div>");
  });

  test("leaves html untouched when the head renders to nothing", () => {
    const html = "<html><head></head><body></body></html>";
    expect(weaveHead(html, {})).toBe(html);
    expect(weaveHead(html, { noindex: false })).toBe(html);
  });
});
