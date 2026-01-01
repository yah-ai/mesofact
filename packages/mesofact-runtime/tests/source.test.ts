// BaseSource per-call override plumbing — `.noTrack()` / `.timeout(ms)` apply
// to exactly the next read, then reset. Tag emission flows into trackCtx.

import { describe, expect, test } from "bun:test";
import {
  BaseSource,
  type BlobSource,
  type R2Object,
  type ListOpts,
  runInTrackCtx,
} from "../src/index.js";

class FakeR2 extends BaseSource implements BlobSource {
  // Records every read's effective {track, timeout_ms} for inspection.
  readonly calls: Array<{ track: boolean; timeout_ms: number }> = [];

  constructor(
    name: string,
    private readonly bucket: string,
    private readonly defaultTimeoutMs = 2000,
  ) {
    super(name);
  }

  async fetch(key: string): Promise<Uint8Array | null> {
    const { track, timeout_ms } = this.consumeOverrides(this.defaultTimeoutMs);
    this.calls.push({ track, timeout_ms });
    this.emitTag(`r2:${this.bucket}:${key}`, track);
    return new Uint8Array(0);
  }

  async list(prefix: string, _opts?: ListOpts): Promise<R2Object[]> {
    const { track, timeout_ms } = this.consumeOverrides(this.defaultTimeoutMs);
    this.calls.push({ track, timeout_ms });
    this.emitTag(`r2:${this.bucket}:${prefix}*`, track);
    return [];
  }
}

describe("BaseSource override plumbing", () => {
  test("default read is tracked with the adapter's default timeout", async () => {
    const adapter = new FakeR2("assets", "yah-assets");
    const { ctx } = await runInTrackCtx(async () => {
      await adapter.fetch("css/app.css");
    });
    expect(adapter.calls).toEqual([{ track: true, timeout_ms: 2000 }]);
    expect([...ctx.tags]).toEqual(["r2:yah-assets:css/app.css"]);
  });

  test(".noTrack() suppresses the very next read and resets after", async () => {
    const adapter = new FakeR2("assets", "yah-assets");
    const { ctx } = await runInTrackCtx(async () => {
      await adapter.noTrack().fetch("flags.json");
      await adapter.fetch("css/app.css");
    });
    expect(adapter.calls[0]).toEqual({ track: false, timeout_ms: 2000 });
    expect(adapter.calls[1]).toEqual({ track: true, timeout_ms: 2000 });
    expect([...ctx.tags]).toEqual(["r2:yah-assets:css/app.css"]);
  });

  test(".timeout(ms) overrides the next read and resets after", async () => {
    const adapter = new FakeR2("assets", "yah-assets");
    await runInTrackCtx(async () => {
      await adapter.timeout(500).fetch("css/app.css");
      await adapter.fetch("css/app.css");
    });
    expect(adapter.calls[0]).toEqual({ track: true, timeout_ms: 500 });
    expect(adapter.calls[1]).toEqual({ track: true, timeout_ms: 2000 });
  });

  test("chaining .noTrack().timeout(ms) applies both to one read", async () => {
    const adapter = new FakeR2("assets", "yah-assets");
    await runInTrackCtx(async () => {
      await adapter.noTrack().timeout(50).fetch("flags.json");
    });
    expect(adapter.calls[0]).toEqual({ track: false, timeout_ms: 50 });
  });

  test("reads outside a trackCtx scope use defaults and emit no tags", async () => {
    const adapter = new FakeR2("assets", "yah-assets");
    await adapter.noTrack().fetch("css/app.css");
    // No ctx → noTrack() is a no-op → consumeOverrides returns defaults.
    expect(adapter.calls[0]).toEqual({ track: true, timeout_ms: 2000 });
  });

  test("list reads emit a prefix tag", async () => {
    const adapter = new FakeR2("assets", "yah-assets");
    const { ctx } = await runInTrackCtx(async () => {
      await adapter.list("css/");
    });
    expect([...ctx.tags]).toEqual(["r2:yah-assets:css/*"]);
  });
});
