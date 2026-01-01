# mesofact-publisher

Walks `dist/`, uploads to R2 under `/{build_id}/`, atomically swaps
`/manifest.json`, and purges CDN tags. See
[`../../.yah/docs/architecture/mesofact.md`](../../.yah/docs/architecture/mesofact.md)
§"Publisher tag-subscription" / §"Static asset handling".
