// `r2` adapter — read-only BlobSource over Cloudflare R2 (or any S3-compatible
// endpoint). Signs requests with SigV4 via `aws4fetch`, emits `r2:<bucket>:<key>`
// tags into the ambient trackCtx, honors per-call `.noTrack()` / `.timeout(ms)`.
//
// See `.yah/docs/architecture/mesofact.md` §"Adapter API surface" + §"Adapter
// read-set provenance".

import { AwsClient } from "aws4fetch";
import { BaseSource, type BlobSource, type ListOpts, type R2Object } from "../source.js";
import { SourceQueryError, SourceTimeoutError, SourceUnavailableError } from "../errors.js";

const DEFAULT_TIMEOUT_MS = 2000;

export type R2Config = {
  // Logical source name from `mesofact.config.toml` (e.g. "assets"). Used as
  // the registry key and surfaced in errors.
  name: string;
  // R2 bucket — also the `r2:<bucket>:...` tag prefix, per the design's tag
  // taxonomy.
  bucket: string;
  // Endpoint root. For Cloudflare R2 this is
  // `https://<account_id>.r2.cloudflarestorage.com`. No trailing slash.
  endpoint: string;
  accessKeyId: string;
  secretAccessKey: string;
  // Test seam — swap `fetch` to inject a stub. Defaults to `globalThis.fetch`,
  // which aws4fetch already uses internally; passing a custom fn lets tests
  // observe the signed request without hitting the network.
  httpFetch?: typeof fetch;
};

export class R2Adapter extends BaseSource implements BlobSource {
  readonly bucket: string;
  private readonly client: AwsClient;
  private readonly endpoint: string;
  private readonly httpFetch: typeof fetch;

  constructor(config: R2Config) {
    super(config.name);
    this.bucket = config.bucket;
    this.endpoint = config.endpoint.replace(/\/$/, "");
    this.client = new AwsClient({
      accessKeyId: config.accessKeyId,
      secretAccessKey: config.secretAccessKey,
      service: "s3",
      region: "auto",
    });
    // aws4fetch always uses globalThis.fetch internally; we sign manually and
    // dispatch through `httpFetch` so tests can swap it without touching the
    // global. Calling `globalThis.fetch.bind(globalThis)` preserves the
    // expected receiver for fetch implementations that check it.
    this.httpFetch = config.httpFetch ?? globalThis.fetch.bind(globalThis);
  }

  async fetch(key: string): Promise<Uint8Array | null> {
    const { track, timeout_ms } = this.consumeOverrides(DEFAULT_TIMEOUT_MS);
    this.emitTag(`r2:${this.bucket}:${key}`, track);
    const url = `${this.endpoint}/${this.bucket}/${encodeKey(key)}`;
    const res = await this.send(url, { method: "GET" }, timeout_ms);
    if (res.status === 404) return null;
    if (!res.ok) {
      throw new SourceQueryError(this.name, `r2 GET ${key} → HTTP ${res.status}`);
    }
    const buf = await res.arrayBuffer();
    return new Uint8Array(buf);
  }

  async list(prefix: string, opts: ListOpts = {}): Promise<R2Object[]> {
    const { track, timeout_ms } = this.consumeOverrides(DEFAULT_TIMEOUT_MS);
    this.emitTag(`r2:${this.bucket}:${prefix}*`, track);
    const params = new URLSearchParams({ "list-type": "2", prefix });
    if (opts.limit !== undefined) params.set("max-keys", String(opts.limit));
    if (opts.cursor) params.set("continuation-token", opts.cursor);
    if (opts.delimiter) params.set("delimiter", opts.delimiter);
    const url = `${this.endpoint}/${this.bucket}?${params.toString()}`;
    const res = await this.send(url, { method: "GET" }, timeout_ms);
    if (!res.ok) {
      throw new SourceQueryError(this.name, `r2 LIST ${prefix} → HTTP ${res.status}`);
    }
    return parseListV2(await res.text());
  }

  private async send(url: string, init: RequestInit, timeout_ms: number): Promise<Response> {
    const signed = await this.client.sign(url, init);
    return this.race(() => this.httpFetch(signed), timeout_ms);
  }

  private async race(call: () => Promise<Response>, timeout_ms: number): Promise<Response> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race<Response>([
        call().catch((err) => {
          throw new SourceUnavailableError(this.name, { cause: err });
        }),
        new Promise<Response>((_, reject) => {
          timer = setTimeout(() => reject(new SourceTimeoutError(this.name, timeout_ms)), timeout_ms);
        }),
      ]);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }
}

// `encodeURIComponent` re-encodes `/`, which we want to preserve so S3 sees
// path-style keys correctly. Other reserved chars stay percent-encoded.
function encodeKey(key: string): string {
  return key
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
}

// Minimal ListBucketResult v2 parser. S3's XML is a fixed shape; a regex over
// the `<Contents>` blocks is faster and dep-free than pulling in a full XML
// parser, and the only fields the contract exposes are key/size/last_modified/
// etag (per `R2Object`).
function parseListV2(xml: string): R2Object[] {
  const out: R2Object[] = [];
  for (const match of xml.matchAll(/<Contents>([\s\S]*?)<\/Contents>/g)) {
    const inner = match[1]!;
    const key = pick(inner, "Key");
    const sizeStr = pick(inner, "Size");
    const last_modified = pick(inner, "LastModified");
    const etagRaw = pick(inner, "ETag");
    if (key === undefined || sizeStr === undefined || last_modified === undefined) continue;
    out.push({
      key,
      size: Number.parseInt(sizeStr, 10),
      last_modified,
      ...(etagRaw ? { etag: etagRaw.replace(/^"|"$/g, "") } : {}),
    });
  }
  return out;
}

function pick(haystack: string, tag: string): string | undefined {
  const m = haystack.match(new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`));
  return m?.[1];
}

// Per-process registry. Configuration in T4 loads `mesofact.config.toml` and
// calls `registerR2()` for each `[sources.*]` of `kind = "r2"`. Render code
// looks up by name via `r2(name)`.
const registry = new Map<string, R2Adapter>();

export function registerR2(adapter: R2Adapter): void {
  registry.set(adapter.name, adapter);
}

export function clearR2Registry(): void {
  registry.clear();
}

export function r2(name: string): BlobSource {
  const adapter = registry.get(name);
  if (!adapter) {
    throw new Error(
      `r2 source not registered: ${name} (declare it in mesofact.config.toml under [sources.${name}])`,
    );
  }
  return adapter;
}
