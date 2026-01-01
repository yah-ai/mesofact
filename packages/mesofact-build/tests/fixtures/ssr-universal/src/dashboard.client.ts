/// <reference lib="dom" />
// Client hydrate entry — reads __mesofact_data__ and renders into #root.
// Framework-free so the fixture has no extra deps; a real consumer would
// call its framework's hydrateRoot here.

type Data = { user: string; count: number; label: string };

function readData(): Data | null {
  const el = document.getElementById("__mesofact_data__");
  if (!el?.textContent) return null;
  return JSON.parse(el.textContent) as Data;
}

function hydrate(): void {
  const data = readData();
  const root = document.getElementById("root");
  if (!root || !data) return;
  root.textContent = `${data.user}: ${data.count}`;
}

hydrate();
