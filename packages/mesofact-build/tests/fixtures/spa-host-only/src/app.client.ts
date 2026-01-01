// The client entry itself is innocuous — it imports a helper that pulls in
// node:fs. The lint must walk transitively and surface the import chain.
import { describe } from "./describe.js";

function hydrate(): void {
  const root = document.getElementById("root");
  if (root) root.textContent = describe();
}

hydrate();
