// The one shared component. Every route below renders inside it, so the three
// modes differ only in *when* the HTML is produced — not in how it's written.

const STYLES = `
  :root { color-scheme: light dark; --fg: #111; --muted: #666; --bg: #fafafa; --accent: #4f46e5; }
  @media (prefers-color-scheme: dark) {
    :root { --fg: #f5f5f5; --muted: #999; --bg: #0a0a0a; --accent: #818cf8; }
  }
  body {
    font: 16px/1.55 system-ui, -apple-system, "Segoe UI", sans-serif;
    color: var(--fg); background: var(--bg);
    max-width: 40rem; margin: 0 auto; padding: 3rem 1.5rem;
  }
  h1 { font-size: 2rem; margin: 0 0 .25rem; }
  .mode { color: var(--muted); font-family: ui-monospace, monospace; font-size: .85rem; }
  nav { margin: 2rem 0 0; display: flex; gap: 1rem; }
  a { color: var(--accent); }
  code { background: rgba(127,127,127,.15); padding: .05em .35em; border-radius: .25em; }
`;

export function Page({
  title,
  mode,
  children,
}: {
  title: string;
  mode: string;
  children?: React.ReactNode;
}) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{title}</title>
        <style dangerouslySetInnerHTML={{ __html: STYLES }} />
      </head>
      <body>
        <h1>{title}</h1>
        <p className="mode">mode: {mode}</p>
        {children}
        <nav>
          <a href="/">/ static</a>
          <a href="/live">/live ssr</a>
          <a href="/app">/app spa</a>
          <a href="/api/hello">/api/hello ssr</a>
        </nav>
      </body>
    </html>
  );
}

/// A full document string, `<!doctype html>` included. React's
/// `renderToStaticMarkup` does not emit the doctype, and a page without one
/// puts the browser in quirks mode — which is the kind of thing you only
/// notice much later, in a layout bug that looks like a CSS problem.
export function documentOf(markup: string): string {
  return `<!doctype html>\n${markup}\n`;
}
