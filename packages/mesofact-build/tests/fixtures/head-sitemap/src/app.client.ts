/// <reference lib="dom" />
// Client hydration entry for the /app shell — framework-free so the fixture
// carries no runtime deps. Its only job here is to exist: a `mode: "spa"`
// route is rejected at config validation without a client_entrypoint.

function hydrate(): void {
  const root = document.getElementById("root");
  if (root) root.textContent = "hydrated";
}

hydrate();
