// Hydration handoff helpers — XSS escape rule + tag shapes (R015-F3 / W173).

import { describe, expect, test } from "bun:test";
import {
  SSR_DATA_SCRIPT_ID,
  escapeJsonForScriptTag,
  hydrationDataTag,
  hydrationScriptTag,
} from "../src/index.js";

describe("escapeJsonForScriptTag", () => {
  test("escapes `<` so a literal </script> in data can't close the tag", () => {
    const encoded = escapeJsonForScriptTag({ note: "</script><img>" });
    expect(encoded).not.toContain("</script>");
    expect(encoded).toContain("\\u003c");
    // Round-trips through JSON.parse with no special handling.
    expect(JSON.parse(encoded)).toEqual({ note: "</script><img>" });
  });

  test("escapes `>` and `&` for defense in depth", () => {
    const encoded = escapeJsonForScriptTag("a > b && c");
    expect(encoded).toContain("\\u003e");
    expect(encoded).toContain("\\u0026");
    expect(JSON.parse(encoded)).toBe("a > b && c");
  });

  test("escapes U+2028 / U+2029 line separators", () => {
    const encoded = escapeJsonForScriptTag("line1 line2 line3");
    expect(encoded).toContain("\\u2028");
    expect(encoded).toContain("\\u2029");
    expect(encoded).not.toContain(" ");
    expect(encoded).not.toContain(" ");
    expect(JSON.parse(encoded)).toBe("line1 line2 line3");
  });

  test("passes innocuous JSON through unchanged structure", () => {
    const encoded = escapeJsonForScriptTag({ a: 1, b: [true, null] });
    expect(JSON.parse(encoded)).toEqual({ a: 1, b: [true, null] });
  });

  test("handles a hostile payload with <!-- and <![CDATA[ markers", () => {
    const payload = { evil: "<!--<script>alert(1)</script>--><![CDATA[oops]]>" };
    const encoded = escapeJsonForScriptTag(payload);
    expect(encoded).not.toContain("<!--");
    expect(encoded).not.toContain("<![CDATA[");
    expect(encoded).not.toContain("<script>");
    expect(JSON.parse(encoded)).toEqual(payload);
  });
});

describe("hydrationDataTag", () => {
  test("uses the W173-pinned __mesofact_data__ id", () => {
    expect(SSR_DATA_SCRIPT_ID).toBe("__mesofact_data__");
    const tag = hydrationDataTag({ ok: true });
    expect(tag).toContain('id="__mesofact_data__"');
    expect(tag).toContain('type="application/json"');
  });

  test("wraps the escaped JSON exactly once and is round-trippable", () => {
    const data = { count: 7, label: "<b>html</b>" };
    const tag = hydrationDataTag(data);
    // Tag boundaries can't be broken by the data.
    expect(tag.startsWith('<script id="__mesofact_data__" type="application/json">')).toBe(true);
    expect(tag.endsWith("</script>")).toBe(true);
    // No raw `<b>` leaks through to the rendered DOM.
    expect(tag).not.toContain("<b>");
    // Extract the JSON portion and round-trip.
    const inner = tag.slice(
      '<script id="__mesofact_data__" type="application/json">'.length,
      -"</script>".length,
    );
    expect(JSON.parse(inner)).toEqual(data);
  });
});

describe("hydrationScriptTag", () => {
  test("emits a module script with the given src", () => {
    expect(hydrationScriptTag("/build123/hydrate/app.abc.js")).toBe(
      '<script type="module" src="/build123/hydrate/app.abc.js"></script>',
    );
  });

  test("escapes `\"`, `<`, `&` in src to protect against attribute breakout", () => {
    const tag = hydrationScriptTag('"/x"><svg onload=alert(1)>');
    expect(tag).not.toContain('"/x">');
    expect(tag).toContain("&quot;");
    expect(tag).toContain("&lt;");
  });
});
