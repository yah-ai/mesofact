/// <reference lib="dom" />
// Client hydration entry — runs in the browser. Reads the build-injected
// __MESOFACT_STATE__ tag and renders into #root. Framework-free so the fixture
// has no runtime deps; a real app would call its framework's hydrateRoot here.

type State = { count: number; label: string };

function readState(): State | null {
  const el = document.getElementById("__MESOFACT_STATE__");
  if (!el?.textContent) return null;
  return JSON.parse(el.textContent) as State;
}

function hydrate(): void {
  const state = readState();
  const root = document.getElementById("root");
  if (!root || !state) return;
  root.textContent = `${state.label}: ${state.count}`;
}

hydrate();
