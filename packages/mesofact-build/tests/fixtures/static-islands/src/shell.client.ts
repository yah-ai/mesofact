/// <reference lib="dom" />
// Client hydration entry — reads __MESOFACT_STATE__ and adds a small count
// badge next to the prerendered list. Framework-free so the fixture has no
// extra deps; a real consumer would call its framework's hydrateRoot here.

type State = { count: number };

function hydrate(): void {
  const el = document.getElementById("__MESOFACT_STATE__");
  if (!el?.textContent) return;
  const state = JSON.parse(el.textContent) as State;
  const list = document.getElementById("issues");
  if (!list || !state) return;
  const badge = document.createElement("span");
  badge.id = "issue-count";
  badge.textContent = `(${state.count})`;
  list.parentNode?.insertBefore(badge, list);
}

hydrate();
