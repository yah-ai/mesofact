// The interactive half of the `spa` route, imported by BOTH sides:
// src/app_shell.tsx renders it at build time, src/app.client.tsx hydrates it in
// the browser. One component, rendered twice — that shared import is what makes
// the two renders agree, and disagreement is the whole failure mode of
// hydration.

import { useState } from "react";

export type AppState = { greeting: string; clicks: number };

// Renders a fragment, NOT the `#root` element. `#root` is the hydration
// CONTAINER — the shell emits it and the client passes it to `hydrateRoot` —
// so a component that rendered it too would nest one inside the other and the
// hydrate would mismatch on the first tag it looked at.
export function App({ initial }: { initial: AppState }) {
  const [clicks, setClicks] = useState(initial.clicks);
  return (
    <>
      <p>{initial.greeting}</p>
      <p>
        <button onClick={() => setClicks((c) => c + 1)}>clicked {clicks}×</button>
      </p>
      <p className="mode">
        The button does nothing until the hydrate bundle loads. That gap is the
        cost of this mode, and being able to see it is the point of the example.
      </p>
    </>
  );
}
