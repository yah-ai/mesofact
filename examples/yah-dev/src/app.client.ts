/// <reference lib="dom" />
// Client hydration entry for the Mode 3 `/app` route. Bundled to
// dist/hydrate/ by @mesofact/build (browser target, content-hashed). It reads
// the build-injected __MESOFACT_STATE__ tag and takes over #root. Framework-
// free so the example carries no runtime deps — a real app would call its
// framework's hydrateRoot() here (see the six-line snippet in contract.ts).

type State = { hydrated_at: string; message: string };

function readState(): State | null {
  const el = document.getElementById("__MESOFACT_STATE__");
  if (!el?.textContent) return null;
  return JSON.parse(el.textContent) as State;
}

function hydrate(): void {
  const state = readState();
  const root = document.getElementById("root");
  if (!root) return;
  root.textContent = state ? state.message : "no state";
}

hydrate();
