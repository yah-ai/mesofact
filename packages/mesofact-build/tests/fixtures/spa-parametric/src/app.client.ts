/// <reference lib="dom" />
// Client entry — reads :id from the URL at runtime, not from build params.
const id = location.pathname.split("/").at(-1) ?? "";
const root = document.getElementById("root");
if (root) root.textContent = `item: ${id}`;
