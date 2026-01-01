export type LayoutOpts = {
  title: string;
  description: string;
  body: string;
};

const STYLES = `
  :root { color-scheme: light dark; --fg: #111; --muted: #555; --bg: #fafafa; --accent: #4f46e5; }
  @media (prefers-color-scheme: dark) {
    :root { --fg: #f5f5f5; --muted: #aaa; --bg: #0a0a0a; --accent: #818cf8; }
  }
  body {
    font: 16px/1.55 system-ui, -apple-system, "Segoe UI", sans-serif;
    color: var(--fg);
    background: var(--bg);
    max-width: 36rem;
    margin: 0 auto;
    padding: 3rem 1.5rem;
  }
  h1 { font-size: 2rem; margin: 0 0 1rem; }
  p { margin: 0 0 1rem; }
  code { background: rgba(127,127,127,0.15); padding: 0.05em 0.35em; border-radius: 0.25em; }
  a { color: var(--accent); }
`;

export function layout({ title, description, body }: LayoutOpts): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(title)}</title>
<meta name="description" content="${escapeHtml(description)}">
<style>${STYLES}</style>
</head>
<body>
${body.trim()}
</body>
</html>
`;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
