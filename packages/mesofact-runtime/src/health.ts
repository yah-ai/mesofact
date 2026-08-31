// `defineReadyz` — the TSX half of mesofact's `/readyz` probe.
//
// mesofact serves `/livez` and `/readyz` itself, in Rust
// (`crates/mesofact/src/health.rs`), gated on the things only the engine can
// know: is the SSR isolate up, is there a tree to serve, are we draining. An
// app opts into contributing its own readiness by declaring the path as an
// ordinary SSR route — there is no config key and no manifest field:
//
//   // mesofact.routes.ts
//   { route: "/readyz", mode: "ssr", entrypoint: "src/readyz.ts", cache_policy: { ttl: 0 } }
//
// The Rust handler notices the app claimed the path, dispatches to it while
// answering the probe, and folds the response status into its own listing as a
// check named `app`. Anything outside 2xx takes the pod out of rotation.
//
// # The contract, and its one asymmetry
//
// An app verdict is **additive**: it can only make the process less ready. A
// 200 from here cannot un-drain a terminating process or overrule a dead
// isolate, because those are exactly the states in which app code is the
// unreliable narrator. Design your checks accordingly — this is the place to
// say "my database pool is exhausted", not "ignore the engine, I'm fine".
//
// # Vanilla, or this helper
//
// The route is a plain Fetch handler, so vanilla works and owes nothing to this
// module:
//
//   export default async () =>
//     (await db.ping()) ? new Response("ok") : new Response("db", { status: 503 });
//
// `defineReadyz` is the opt-in ergonomic layer: it runs named checks and emits
// the *same* wire format the Rust side emits — the kube-apiserver `[+]name ok`
// listing under `?verbose`. Matching formats is the point. An operator running
// `curl localhost:3000/readyz?verbose` should not be able to tell which
// language answered, and when the Rust side aggregates, the two listings nest
// instead of clashing.

/** One named readiness condition. `check` should be fast and side-effect free —
 *  probes run on the kubelet's schedule, per replica. */
export type ReadyCheck = {
  /** Operator-facing name in `?verbose` output. A noun (`db`, `cache`). */
  name: string;
  check: () => boolean | Promise<boolean>;
};

/** Build the Fetch handler for a `mode:"ssr"` `/readyz` route.
 *
 *  Every check runs, even after one fails: reporting only the first failure
 *  hides a second broken subsystem behind the first. A check that throws counts
 *  as failed — an exception is not an assertion of readiness. */
export function defineReadyz(
  checks: readonly ReadyCheck[],
): (req: Request) => Promise<Response> {
  return async (req: Request): Promise<Response> => {
    const results = await Promise.all(
      checks.map(async (c) => {
        try {
          return { name: c.name, pass: (await c.check()) === true };
        } catch {
          return { name: c.name, pass: false };
        }
      }),
    );

    const ok = results.every((r) => r.pass);
    const status = ok ? 200 : 503;
    const headers = {
      "content-type": "text/plain; charset=utf-8",
      // A cached 200 outlives the condition it described, which is the exact
      // failure the probe exists to catch.
      "cache-control": "no-cache, no-store, must-revalidate",
    };

    if (!isVerbose(req)) {
      return new Response(ok ? "ok\n" : "readyz check failed\n", {
        status,
        headers,
      });
    }

    const listing = results
      .map((r) => (r.pass ? `[+]${r.name} ok\n` : `[-]${r.name} failed\n`))
      .join("");
    const trailer = ok ? "readyz check passed\n" : "readyz check failed\n";
    return new Response(listing + trailer, { status, headers });
  };
}

/** `?verbose` / `?verbose=1`, matching the Rust `is_verbose`. A malformed URL
 *  means the non-verbose body, never a crash — a probe that 500s because it
 *  could not parse its own URL is worse than one that answers tersely. */
function isVerbose(req: Request): boolean {
  try {
    return new URL(req.url).searchParams.has("verbose");
  } catch {
    return false;
  }
}
