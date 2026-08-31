import { describe, expect, test } from "bun:test";
import { defineReadyz } from "../src/health.js";

const get = (url = "http://x/readyz") => new Request(url);

describe("defineReadyz", () => {
  test("no checks is ready", async () => {
    const res = await defineReadyz([])(get());
    expect(res.status).toBe(200);
    expect(await res.text()).toBe("ok\n");
  });

  test("a failing check is 503", async () => {
    const res = await defineReadyz([{ name: "db", check: () => false }])(get());
    expect(res.status).toBe(503);
    expect(await res.text()).toBe("readyz check failed\n");
  });

  test("verbose lists every check, not just the first failure", async () => {
    // Reporting only the first failure hides a second broken subsystem behind
    // the first — the same reason the Rust side evaluates all of them.
    const handler = defineReadyz([
      { name: "db", check: () => false },
      { name: "cache", check: () => false },
    ]);
    const res = await handler(get("http://x/readyz?verbose"));

    expect(res.status).toBe(503);
    expect(await res.text()).toBe(
      "[-]db failed\n[-]cache failed\nreadyz check failed\n",
    );
  });

  test("the verbose listing matches the Rust wire format", async () => {
    // `crates/mesofact/src/health.rs` emits exactly this shape. An operator
    // must not be able to tell which language answered.
    const res = await defineReadyz([{ name: "db", check: () => true }])(
      get("http://x/readyz?verbose"),
    );
    expect(await res.text()).toBe("[+]db ok\nreadyz check passed\n");
    expect(res.headers.get("cache-control")).toBe(
      "no-cache, no-store, must-revalidate",
    );
  });

  test("an async check is awaited", async () => {
    const res = await defineReadyz([
      { name: "db", check: async () => true },
    ])(get());
    expect(res.status).toBe(200);
  });

  test("a throwing check counts as failed, not as an error response", async () => {
    // An exception is not an assertion of readiness — but a probe that 500s is
    // less useful than one that says which check blew up.
    const handler = defineReadyz([
      {
        name: "db",
        check: () => {
          throw new Error("pool exhausted");
        },
      },
    ]);
    const res = await handler(get("http://x/readyz?verbose"));

    expect(res.status).toBe(503);
    expect(await res.text()).toContain("[-]db failed");
  });

  test("a truthy non-true check value does not count as ready", async () => {
    const handler = defineReadyz([
      { name: "db", check: () => "yes" as unknown as boolean },
    ]);
    expect((await handler(get())).status).toBe(503);
  });
});
