// Lean Fetch surface for the SSR isolate (R750-F1, W225 §2b). Pure JS over
// one Rust op (`op_mesofact_fetch`, src/ops_fetch.rs) — replaces the
// deno_web + deno_fetch extension stack and the TLS/QUIC/DNS closure it
// dragged in. Installs: URL, URLSearchParams, Headers, Request, Response,
// fetch. TextEncoder/TextDecoder come from the shared SSG polyfill
// (js/bootstrap.js), which the SSR isolate runs too.
//
// Scope is the measured surface (W225 §2b Finding 1 + 2) plus the obvious
// neighbours of each member. Deliberately NOT supported:
//   - URL: IDN / punycode hosts, IPv6 normalisation, blob: / data: /
//     file: semantics (non-http(s) schemes parse as opaque `scheme:rest`),
//     username/password setters.
//   - Bodies are buffered Uint8Arrays, never ReadableStreams: `Response.body`
//     is a Uint8Array (or null). Passing it straight back into
//     `new Response(upstream.body, ...)` — the one streaming pass-through the
//     probe saw — works because the constructor accepts Uint8Array.
//   - No FormData / Blob bodies, no AbortSignal, no redirect/credentials/mode.
"use strict";
((globalThis) => {
  const ops = globalThis.Deno.core.ops;

  // ---------------------------------------------------------------- URL ---

  const SPECIAL_PORTS = { "http:": "80", "https:": "443", "ws:": "80", "wss:": "443" };
  const SCHEME_RE = /^([a-zA-Z][a-zA-Z\d+.-]*):/;
  const AUTHORITY_RE = /^\/\/([^/?#]*)/;

  // Percent-encode anything outside the given safe set, leaving existing
  // %XX escapes alone.
  const encodeSet = (s, safe) =>
    s.replace(/[^\x00-\x7f]|[\x00-\x20"<>`{}|\\^\x7f]/g, (ch) => (safe.includes(ch) ? ch : encodeURIComponent(ch)));
  const encPath = (s) => encodeSet(s, "");
  const encQuery = (s) => encodeSet(s, "`{}|\\^");
  const encHash = (s) => encodeSet(s, "<>{}|\\^");

  function removeDotSegments(path) {
    const out = [];
    const segs = path.split("/");
    for (let i = 0; i < segs.length; i++) {
      const seg = segs[i];
      if (seg === "..") {
        if (out.length > 1) out.pop();
        if (i === segs.length - 1) out.push("");
      } else if (seg === ".") {
        if (i === segs.length - 1) out.push("");
      } else {
        out.push(seg);
      }
    }
    const joined = out.join("/");
    return joined.startsWith("/") ? joined : "/" + joined;
  }

  // Split `rest` (everything after `scheme:`) into {authority, path, query, hash}.
  function splitRest(rest) {
    let hash = "";
    let query = "";
    const h = rest.indexOf("#");
    if (h >= 0) {
      hash = rest.slice(h + 1);
      rest = rest.slice(0, h);
    }
    const q = rest.indexOf("?");
    if (q >= 0) {
      query = rest.slice(q + 1);
      rest = rest.slice(0, q);
    }
    let authority = null;
    const m = AUTHORITY_RE.exec(rest);
    if (m) {
      authority = m[1];
      rest = rest.slice(m[0].length);
    }
    return { authority, path: rest, query: q >= 0 ? query : null, hash: h >= 0 ? hash : null };
  }

  function parseAuthority(protocol, authority) {
    const at = authority.lastIndexOf("@");
    let userinfo = "";
    let hostport = authority;
    if (at >= 0) {
      userinfo = authority.slice(0, at);
      hostport = authority.slice(at + 1);
    }
    const c = userinfo.indexOf(":");
    const username = c >= 0 ? userinfo.slice(0, c) : userinfo;
    const password = c >= 0 ? userinfo.slice(c + 1) : "";
    let hostname = hostport;
    let port = "";
    const pm = /:(\d*)$/.exec(hostport);
    if (pm && !hostport.endsWith("]")) {
      hostname = hostport.slice(0, pm.index);
      port = pm[1];
    }
    if (hostname === "") throw new TypeError(`Invalid URL: empty host`);
    if (port !== "" && Number(port) > 65535) throw new TypeError(`Invalid URL: port ${port}`);
    if (SPECIAL_PORTS[protocol] === port) port = "";
    return { username, password, hostname: hostname.toLowerCase(), port };
  }

  class URL {
    #protocol = "";
    #username = "";
    #password = "";
    #hostname = "";
    #port = "";
    #pathname = "";
    #search = "";
    #hash = "";
    #opaque = false;
    #params = null;

    constructor(input, base) {
      input = String(input).trim();
      const sm = SCHEME_RE.exec(input);
      if (sm) {
        this.#parseAbsolute(sm[1].toLowerCase() + ":", input.slice(sm[0].length));
      } else {
        if (base === undefined) throw new TypeError(`Invalid URL: '${input}'`);
        const b = base instanceof URL ? base : new URL(base);
        this.#resolve(b, input);
      }
    }

    static canParse(input, base) {
      try {
        new URL(input, base);
        return true;
      } catch {
        return false;
      }
    }

    #parseAbsolute(protocol, rest) {
      this.#protocol = protocol;
      const special = protocol in SPECIAL_PORTS;
      if (special) rest = rest.replace(/\\/g, "/");
      const parts = splitRest(rest);
      if (special) {
        if (parts.authority === null) throw new TypeError(`Invalid URL: ${protocol} needs a host`);
        const a = parseAuthority(protocol, parts.authority);
        this.#username = a.username;
        this.#password = a.password;
        this.#hostname = a.hostname;
        this.#port = a.port;
        this.#pathname = encPath(removeDotSegments(parts.path || "/"));
      } else {
        this.#opaque = parts.authority === null;
        if (!this.#opaque) {
          const a = parseAuthority(protocol, parts.authority);
          this.#hostname = a.hostname;
          this.#port = a.port;
        }
        this.#pathname = parts.path;
      }
      this.#search = parts.query ? "?" + encQuery(parts.query) : "";
      this.#hash = parts.hash ? "#" + encHash(parts.hash) : "";
    }

    #resolve(b, input) {
      if (b.#opaque) throw new TypeError(`Invalid URL: cannot resolve '${input}' against ${b.href}`);
      const special = b.#protocol in SPECIAL_PORTS;
      if (special) input = input.replace(/\\/g, "/");
      if (input.startsWith("//")) {
        this.#parseAbsolute(b.#protocol, input);
        return;
      }
      this.#protocol = b.#protocol;
      this.#username = b.#username;
      this.#password = b.#password;
      this.#hostname = b.#hostname;
      this.#port = b.#port;
      const parts = splitRest(input);
      if (parts.path === "") {
        this.#pathname = b.#pathname;
        this.#search = parts.query !== null ? (parts.query ? "?" + encQuery(parts.query) : "") : b.#search;
      } else {
        const path = parts.path.startsWith("/")
          ? parts.path
          : b.#pathname.slice(0, b.#pathname.lastIndexOf("/") + 1) + parts.path;
        this.#pathname = encPath(removeDotSegments(path));
        this.#search = parts.query ? "?" + encQuery(parts.query) : "";
      }
      this.#hash = parts.hash ? "#" + encHash(parts.hash) : "";
    }

    get protocol() { return this.#protocol; }
    get username() { return this.#username; }
    get password() { return this.#password; }
    get hostname() { return this.#hostname; }
    get port() { return this.#port; }
    get host() { return this.#port ? `${this.#hostname}:${this.#port}` : this.#hostname; }
    get origin() {
      return this.#protocol in SPECIAL_PORTS ? `${this.#protocol}//${this.host}` : "null";
    }
    get pathname() { return this.#pathname; }
    set pathname(v) {
      if (this.#opaque) return;
      v = String(v);
      this.#pathname = encPath(removeDotSegments(v.startsWith("/") ? v : "/" + v));
    }
    get search() { return this.#search.length > 1 ? this.#search : ""; }
    set search(v) {
      v = String(v);
      if (v.startsWith("?")) v = v.slice(1);
      this.#search = v ? "?" + encQuery(v) : "";
      if (this.#params) this.#params._reset(v);
    }
    get hash() { return this.#hash.length > 1 ? this.#hash : ""; }
    set hash(v) {
      v = String(v);
      if (v.startsWith("#")) v = v.slice(1);
      this.#hash = v ? "#" + encHash(v) : "";
    }
    get searchParams() {
      if (!this.#params) {
        this.#params = new URLSearchParams(this.search);
        this.#params._owner = (s) => {
          this.#search = s ? "?" + s : "";
        };
      }
      return this.#params;
    }
    get href() {
      if (this.#opaque) return `${this.#protocol}${this.#pathname}${this.#search}${this.#hash}`;
      const cred = this.#username
        ? this.#username + (this.#password ? ":" + this.#password : "") + "@"
        : "";
      return `${this.#protocol}//${cred}${this.host}${this.#pathname}${this.#search}${this.#hash}`;
    }
    set href(v) {
      const u = new URL(v);
      this.#protocol = u.#protocol;
      this.#username = u.#username;
      this.#password = u.#password;
      this.#hostname = u.#hostname;
      this.#port = u.#port;
      this.#pathname = u.#pathname;
      this.#search = u.#search;
      this.#hash = u.#hash;
      this.#opaque = u.#opaque;
      if (this.#params) this.#params._reset(this.search.slice(1));
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
  }

  // application/x-www-form-urlencoded: encodeURIComponent minus its extra
  // safe set (!'()~), space as '+'.
  const formEncode = (s) =>
    encodeURIComponent(s)
      .replace(/[!'()~]/g, (c) => "%" + c.charCodeAt(0).toString(16).toUpperCase())
      .replace(/%20/g, "+");
  const formDecode = (s) => {
    s = s.replace(/\+/g, " ");
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  };

  class URLSearchParams {
    #list = [];

    constructor(init = "") {
      if (init instanceof URLSearchParams) {
        this.#list = init.#list.map(([k, v]) => [k, v]);
      } else if (init !== null && typeof init === "object") {
        if (typeof init[Symbol.iterator] === "function") {
          for (const pair of init) {
            const p = [...pair];
            if (p.length !== 2) throw new TypeError("URLSearchParams: each pair must have 2 items");
            this.#list.push([String(p[0]), String(p[1])]);
          }
        } else {
          for (const k of Object.keys(init)) this.#list.push([k, String(init[k])]);
        }
      } else {
        this._reset(String(init));
      }
    }

    _reset(s) {
      if (s.startsWith("?")) s = s.slice(1);
      this.#list = [];
      for (const part of s.split("&")) {
        if (part === "") continue;
        const eq = part.indexOf("=");
        this.#list.push(
          eq >= 0 ? [formDecode(part.slice(0, eq)), formDecode(part.slice(eq + 1))] : [formDecode(part), ""],
        );
      }
    }

    #update() {
      if (this._owner) this._owner(this.toString());
    }

    get size() { return this.#list.length; }
    append(k, v) {
      this.#list.push([String(k), String(v)]);
      this.#update();
    }
    delete(k, v) {
      k = String(k);
      this.#list = this.#list.filter(([lk, lv]) => lk !== k || (v !== undefined && lv !== String(v)));
      this.#update();
    }
    get(k) {
      k = String(k);
      const e = this.#list.find(([lk]) => lk === k);
      return e ? e[1] : null;
    }
    getAll(k) {
      k = String(k);
      return this.#list.filter(([lk]) => lk === k).map(([, v]) => v);
    }
    has(k, v) {
      k = String(k);
      return this.#list.some(([lk, lv]) => lk === k && (v === undefined || lv === String(v)));
    }
    set(k, v) {
      k = String(k);
      v = String(v);
      const i = this.#list.findIndex(([lk]) => lk === k);
      if (i < 0) {
        this.#list.push([k, v]);
      } else {
        this.#list[i][1] = v;
        this.#list = this.#list.filter(([lk], j) => j <= i || lk !== k);
      }
      this.#update();
    }
    sort() {
      this.#list.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      this.#update();
    }
    forEach(cb, thisArg) {
      for (const [k, v] of this.#list) cb.call(thisArg, v, k, this);
    }
    *entries() {
      for (const [k, v] of this.#list) yield [k, v];
    }
    *keys() {
      for (const [k] of this.#list) yield k;
    }
    *values() {
      for (const [, v] of this.#list) yield v;
    }
    [Symbol.iterator]() { return this.entries(); }
    toString() {
      return this.#list.map(([k, v]) => `${formEncode(k)}=${formEncode(v)}`).join("&");
    }
  }

  // ------------------------------------------------------------ Headers ---

  class Headers {
    // lowercased name -> [values]
    #map = new Map();

    constructor(init) {
      if (init === undefined || init === null) return;
      if (init instanceof Headers) {
        for (const [k, vs] of init.#map) this.#map.set(k, [...vs]);
      } else if (typeof init[Symbol.iterator] === "function") {
        for (const pair of init) {
          const p = [...pair];
          if (p.length !== 2) throw new TypeError("Headers: each pair must have 2 items");
          this.append(p[0], p[1]);
        }
      } else if (typeof init === "object") {
        for (const k of Object.keys(init)) this.append(k, init[k]);
      } else {
        throw new TypeError("Headers: init must be an object, array of pairs, or Headers");
      }
    }

    static #name(name) {
      name = String(name);
      if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name)) {
        throw new TypeError(`Headers: invalid header name '${name}'`);
      }
      return name.toLowerCase();
    }
    static #value(v) {
      return String(v).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "");
    }

    append(name, value) {
      const k = Headers.#name(name);
      const v = Headers.#value(value);
      const cur = this.#map.get(k);
      if (cur) cur.push(v);
      else this.#map.set(k, [v]);
    }
    delete(name) { this.#map.delete(Headers.#name(name)); }
    get(name) {
      const vs = this.#map.get(Headers.#name(name));
      return vs ? vs.join(", ") : null;
    }
    getSetCookie() { return [...(this.#map.get("set-cookie") ?? [])]; }
    has(name) { return this.#map.has(Headers.#name(name)); }
    set(name, value) { this.#map.set(Headers.#name(name), [Headers.#value(value)]); }
    *entries() {
      for (const k of [...this.#map.keys()].sort()) {
        const vs = this.#map.get(k);
        if (k === "set-cookie") for (const v of vs) yield [k, v];
        else yield [k, vs.join(", ")];
      }
    }
    *keys() {
      for (const [k] of this.entries()) yield k;
    }
    *values() {
      for (const [, v] of this.entries()) yield v;
    }
    forEach(cb, thisArg) {
      for (const [k, v] of this.entries()) cb.call(thisArg, v, k, this);
    }
    [Symbol.iterator]() { return this.entries(); }
  }

  // --------------------------------------------------------------- Body ---

  // Normalise a BodyInit to {bytes, type}. `type` is the implied
  // content-type, applied only if the caller did not set one.
  function extractBody(body) {
    if (body === undefined || body === null) return { bytes: null, type: null };
    if (typeof body === "string") {
      return { bytes: new TextEncoder().encode(body), type: "text/plain;charset=UTF-8" };
    }
    if (body instanceof URLSearchParams) {
      return {
        bytes: new TextEncoder().encode(body.toString()),
        type: "application/x-www-form-urlencoded;charset=UTF-8",
      };
    }
    if (body instanceof Uint8Array) return { bytes: body, type: null };
    if (body instanceof ArrayBuffer) return { bytes: new Uint8Array(body), type: null };
    if (ArrayBuffer.isView(body)) {
      return { bytes: new Uint8Array(body.buffer, body.byteOffset, body.byteLength), type: null };
    }
    throw new TypeError(
      "mesofact SSR: unsupported body type (string, Uint8Array, ArrayBuffer, " +
        "ArrayBufferView or URLSearchParams only — no ReadableStream/Blob/FormData)",
    );
  }

  // Shared consumption methods for Request/Response. `#bytes`-style private
  // fields cannot be shared across classes, so both keep body state in a
  // symbol-keyed slot.
  const kBody = Symbol("body");
  const kUsed = Symbol("bodyUsed");
  const BodyMixin = {
    get body() { return this[kBody]; },
    get bodyUsed() { return this[kUsed]; },
    async arrayBuffer() {
      const b = consume(this);
      return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
    },
    async bytes() { return consume(this).slice(); },
    async text() { return new TextDecoder().decode(consume(this)); },
    async json() { return JSON.parse(new TextDecoder().decode(consume(this))); },
  };
  function consume(obj) {
    if (obj[kUsed]) throw new TypeError("Body has already been consumed");
    obj[kUsed] = true;
    return obj[kBody] ?? new Uint8Array(0);
  }
  function mixBody(cls) {
    for (const key of Object.getOwnPropertyNames(BodyMixin)) {
      Object.defineProperty(cls.prototype, key, Object.getOwnPropertyDescriptor(BodyMixin, key));
    }
  }

  // ------------------------------------------------------------ Request ---

  class Request {
    #url;
    #method;
    #headers;

    constructor(input, init = {}) {
      let src = null;
      if (input instanceof Request) {
        src = input;
        this.#url = input.url;
      } else {
        this.#url = new URL(String(input)).href;
      }
      let method = init.method ?? src?.method ?? "GET";
      const upper = String(method).toUpperCase();
      method = ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH"].includes(upper)
        ? upper
        : String(method);
      this.#method = method;
      this.#headers = new Headers(init.headers ?? src?.headers);
      let bytes = null;
      if (init.body !== undefined && init.body !== null) {
        if (method === "GET" || method === "HEAD") {
          throw new TypeError("Request with GET/HEAD method cannot have body");
        }
        const ex = extractBody(init.body);
        bytes = ex.bytes;
        if (ex.type && !this.#headers.has("content-type")) this.#headers.set("content-type", ex.type);
      } else if (src) {
        bytes = src[kBody];
      }
      this[kBody] = bytes;
      this[kUsed] = false;
    }

    get url() { return this.#url; }
    get method() { return this.#method; }
    get headers() { return this.#headers; }
    clone() {
      if (this[kUsed]) throw new TypeError("Request body has already been consumed");
      return new Request(this);
    }
  }
  mixBody(Request);

  // ----------------------------------------------------------- Response ---

  class Response {
    #status;
    #statusText;
    #headers;
    #url = "";

    constructor(body = null, init = {}) {
      const status = init.status ?? 200;
      if (!Number.isInteger(status) || status < 200 || status > 599) {
        throw new RangeError(`Response status must be 200..599, got ${status}`);
      }
      this.#status = status;
      this.#statusText = String(init.statusText ?? "");
      this.#headers = new Headers(init.headers);
      const ex = extractBody(body);
      if (ex.bytes !== null && [101, 204, 205, 304].includes(status)) {
        throw new TypeError(`Response with null-body status ${status} cannot have a body`);
      }
      if (ex.type && !this.#headers.has("content-type")) this.#headers.set("content-type", ex.type);
      this[kBody] = ex.bytes;
      this[kUsed] = false;
    }

    static json(data, init = {}) {
      const res = new Response(JSON.stringify(data), init);
      if (!init.headers || !new Headers(init.headers).has("content-type")) {
        res.headers.set("content-type", "application/json");
      }
      return res;
    }
    static redirect(url, status = 302) {
      if (![301, 302, 303, 307, 308].includes(status)) throw new RangeError(`Invalid redirect status ${status}`);
      return new Response(null, { status, headers: { location: new URL(String(url)).href } });
    }

    get status() { return this.#status; }
    get ok() { return this.#status >= 200 && this.#status < 300; }
    get statusText() { return this.#statusText; }
    get headers() { return this.#headers; }
    get url() { return this.#url; }
    get redirected() { return false; }
    get type() { return "default"; }
    clone() {
      if (this[kUsed]) throw new TypeError("Response body has already been consumed");
      const r = new Response(this[kBody], {
        status: this.#status,
        statusText: this.#statusText,
        headers: this.#headers,
      });
      r.#url = this.#url;
      return r;
    }

    // Internal: build from an op_mesofact_fetch reply. Bypasses the 200..599
    // ctor check (upstreams can legitimately answer 1xx-free odd codes) and
    // the null-body rule (a 204 reply simply has an empty buffer).
    static _fromOp(reply, url) {
      const r = new Response(null, { headers: reply.headers });
      r.#status = reply.status;
      r.#statusText = reply.status_text;
      r.#url = url;
      r[kBody] = reply.body;
      return r;
    }
  }
  mixBody(Response);

  // -------------------------------------------------------------- fetch ---

  async function fetch(input, init = {}) {
    const req = new Request(input, init);
    const reply = await ops.op_mesofact_fetch({
      url: req.url,
      method: req.method,
      headers: [...req.headers],
      body: req.body,
    });
    return Response._fromOp(reply, req.url);
  }

  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;
  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.Response = Response;
  globalThis.fetch = fetch;
})(globalThis);
