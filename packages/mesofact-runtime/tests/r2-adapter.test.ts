// R2 adapter — exercises fetch/list against a stubbed S3-compat endpoint.
// Verifies request shape (signed via aws4fetch), tag emission, 404 → null,
// XML list parsing, timeout, and registry factory.

import { afterEach, describe, expect, test } from "bun:test";
import {
  R2Adapter,
  clearR2Registry,
  r2,
  registerR2,
  runInTrackCtx,
  SourceTimeoutError,
  SourceUnavailableError,
} from "../src/index.js";

type Call = { url: string; method: string; headers: Record<string, string> };

function stubFetch(handler: (call: Call) => Response | Promise<Response>): {
  fetch: typeof fetch;
  calls: Call[];
} {
  const calls: Call[] = [];
  const fn: typeof fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const method =
      (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
    const hdrs: Record<string, string> = {};
    const h = init?.headers ?? (input instanceof Request ? input.headers : undefined);
    if (h instanceof Headers) {
      h.forEach((v, k) => {
        hdrs[k.toLowerCase()] = v;
      });
    } else if (Array.isArray(h)) {
      for (const [k, v] of h) hdrs[k.toLowerCase()] = v;
    } else if (h && typeof h === "object") {
      for (const [k, v] of Object.entries(h)) hdrs[k.toLowerCase()] = String(v);
    }
    const call: Call = { url, method, headers: hdrs };
    calls.push(call);
    return handler(call);
  };
  return { fetch: fn, calls };
}

function makeAdapter(opts: {
  name?: string;
  bucket?: string;
  endpoint?: string;
  httpFetch: typeof fetch;
}): R2Adapter {
  return new R2Adapter({
    name: opts.name ?? "assets",
    bucket: opts.bucket ?? "yah-assets",
    endpoint: opts.endpoint ?? "https://acct.r2.cloudflarestorage.com",
    accessKeyId: "AKIAIOSFODNN7EXAMPLE",
    secretAccessKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    httpFetch: opts.httpFetch,
  });
}

afterEach(() => clearR2Registry());

describe("R2Adapter.fetch", () => {
  test("GETs <endpoint>/<bucket>/<key> and returns the body bytes", async () => {
    const { fetch: f, calls } = stubFetch(
      async () => new Response(new Uint8Array([1, 2, 3]), { status: 200 }),
    );
    const adapter = makeAdapter({ httpFetch: f });
    const { value, ctx } = await runInTrackCtx(async () => adapter.fetch("css/app.css"));
    expect(value).toEqual(new Uint8Array([1, 2, 3]));
    expect(calls).toHaveLength(1);
    expect(calls[0]!.method).toBe("GET");
    expect(calls[0]!.url).toBe("https://acct.r2.cloudflarestorage.com/yah-assets/css/app.css");
    expect(calls[0]!.headers["authorization"]).toMatch(/^AWS4-HMAC-SHA256 /);
    expect([...ctx.tags]).toEqual(["r2:yah-assets:css/app.css"]);
  });

  test("404 returns null without erroring", async () => {
    const { fetch: f } = stubFetch(async () => new Response("not found", { status: 404 }));
    const adapter = makeAdapter({ httpFetch: f });
    const out = await runInTrackCtx(async () => adapter.fetch("missing.css"));
    expect(out.value).toBeNull();
  });

  test("5xx throws SourceQueryError carrying the source name", async () => {
    const { fetch: f } = stubFetch(async () => new Response("oops", { status: 503 }));
    const adapter = makeAdapter({ httpFetch: f });
    await expect(runInTrackCtx(async () => adapter.fetch("x.css"))).rejects.toMatchObject({
      name: "SourceQueryError",
      source: "assets",
    });
  });

  test("network failure throws SourceUnavailableError (retryable=true)", async () => {
    const { fetch: f } = stubFetch(async () => {
      throw new TypeError("ECONNREFUSED");
    });
    const adapter = makeAdapter({ httpFetch: f });
    const promise = runInTrackCtx(async () => adapter.fetch("x.css"));
    await expect(promise).rejects.toMatchObject({
      name: "SourceUnavailableError",
      source: "assets",
      retryable: true,
    });
    await expect(promise).rejects.toBeInstanceOf(SourceUnavailableError);
  });

  test(".noTrack() suppresses tag emission for the next call only", async () => {
    const { fetch: f } = stubFetch(async () => new Response("ok", { status: 200 }));
    const adapter = makeAdapter({ httpFetch: f });
    const { ctx } = await runInTrackCtx(async () => {
      await adapter.noTrack().fetch("flags.json");
      await adapter.fetch("css/app.css");
    });
    expect([...ctx.tags]).toEqual(["r2:yah-assets:css/app.css"]);
  });

  test(".timeout(ms) fires before the default when the upstream stalls", async () => {
    const { fetch: f } = stubFetch(
      () =>
        new Promise<Response>(() => {
          // never resolves
        }),
    );
    const adapter = makeAdapter({ httpFetch: f });
    const start = Date.now();
    await expect(
      runInTrackCtx(async () => adapter.timeout(50).fetch("css/app.css")),
    ).rejects.toBeInstanceOf(SourceTimeoutError);
    expect(Date.now() - start).toBeLessThan(500);
  });

  test("path segments containing reserved chars are percent-encoded but / is preserved", async () => {
    const { fetch: f, calls } = stubFetch(async () => new Response("", { status: 200 }));
    const adapter = makeAdapter({ httpFetch: f });
    await runInTrackCtx(async () => adapter.fetch("a b/c?d.txt"));
    expect(calls[0]!.url).toBe(
      "https://acct.r2.cloudflarestorage.com/yah-assets/a%20b/c%3Fd.txt",
    );
  });
});

describe("R2Adapter.list", () => {
  const LIST_XML = `<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>yah-assets</Name>
  <Prefix>css/</Prefix>
  <KeyCount>2</KeyCount>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>css/app.HASH.css</Key>
    <LastModified>2026-05-15T10:00:00.000Z</LastModified>
    <ETag>"abc123"</ETag>
    <Size>1024</Size>
  </Contents>
  <Contents>
    <Key>css/print.css</Key>
    <LastModified>2026-05-14T09:00:00.000Z</LastModified>
    <ETag>"def456"</ETag>
    <Size>256</Size>
  </Contents>
</ListBucketResult>`;

  test("parses ListBucketResult v2 into R2Object[] and emits prefix tag", async () => {
    const { fetch: f, calls } = stubFetch(async () => new Response(LIST_XML, { status: 200 }));
    const adapter = makeAdapter({ httpFetch: f });
    const { value, ctx } = await runInTrackCtx(async () => adapter.list("css/", { limit: 100 }));
    expect(value).toEqual([
      { key: "css/app.HASH.css", size: 1024, last_modified: "2026-05-15T10:00:00.000Z", etag: "abc123" },
      { key: "css/print.css", size: 256, last_modified: "2026-05-14T09:00:00.000Z", etag: "def456" },
    ]);
    expect([...ctx.tags]).toEqual(["r2:yah-assets:css/*"]);
    expect(calls[0]!.url).toContain("list-type=2");
    expect(calls[0]!.url).toContain("prefix=css%2F");
    expect(calls[0]!.url).toContain("max-keys=100");
  });

  test("forwards cursor + delimiter as continuation-token + delimiter", async () => {
    const { fetch: f, calls } = stubFetch(async () => new Response(LIST_XML, { status: 200 }));
    const adapter = makeAdapter({ httpFetch: f });
    await runInTrackCtx(async () => adapter.list("css/", { cursor: "TOKEN1", delimiter: "/" }));
    expect(calls[0]!.url).toContain("continuation-token=TOKEN1");
    expect(calls[0]!.url).toContain("delimiter=%2F");
  });

  test("empty result returns []", async () => {
    const empty = `<?xml version="1.0"?><ListBucketResult><KeyCount>0</KeyCount></ListBucketResult>`;
    const { fetch: f } = stubFetch(async () => new Response(empty, { status: 200 }));
    const adapter = makeAdapter({ httpFetch: f });
    const { value } = await runInTrackCtx(async () => adapter.list("nothing/"));
    expect(value).toEqual([]);
  });
});

describe("r2 registry / factory", () => {
  test("r2(name) returns the registered adapter and threads through it", async () => {
    const { fetch: f } = stubFetch(async () => new Response("body", { status: 200 }));
    registerR2(makeAdapter({ name: "assets", httpFetch: f }));
    const { ctx } = await runInTrackCtx(async () => r2("assets").fetch("k.css"));
    expect([...ctx.tags]).toEqual(["r2:yah-assets:k.css"]);
  });

  test("r2(unknown) throws a name-bearing error", () => {
    expect(() => r2("nope")).toThrow(/r2 source not registered: nope/);
  });
});
