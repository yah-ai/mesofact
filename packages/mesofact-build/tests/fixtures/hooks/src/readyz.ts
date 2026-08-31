import { defineReadyz } from "@mesofact/runtime";

// The hook module's contract is unchanged by the declaration site — still the
// Fetch handler `defineReadyz` returns, because W311's non-goals put Mode 1's
// contract (and this helper's) out of scope.
export default defineReadyz([{ name: "fixture", check: () => true }]);
