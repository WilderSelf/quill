# 0023 — Populate the proxy cache from a document's linked assets

- **Milestone:** M1
- **Status:** in-progress (increment 1 — content-sniffing entry + populate-from-assets)
- **Crates:** `quill-render` (owner), `quill-core-model`

## Problem

Specs 0021 / 0022 gave the `render` `ProxyCache` real proxy pixels for PNG and JPEG, but only via
**format-specific** entries (`insert_png` / `insert_jpeg`) fed raw bytes. Nothing wires the cache to a
**`Document`'s linked assets**: an `Asset` records a `path` (relative, e.g. `assets/map1.png`) and a
format known only by the file's content, not its Rust call site. To actually keep 500-page books fast,
the renderer must be able to take a document's asset list, read each linked file, and populate the
cache with a proxy — the "linked images w/ proxy cache" the plan names.

## Architectural decision (content-sniff + a read-only pass over assets; no new external dep)

Mirror `export-pdf`'s image front-end (`images::resolve` → `images::decode`), which resolves
`base_dir.join(asset.path)`, reads the bytes, and dispatches on the leading magic bytes:

- Add a **content-sniffing** `decode_image_proxy(bytes)` that dispatches PNG-signature → PNG proxy,
  JPEG-SOI → JPEG proxy (the same `\x89PNG…` / `\xFF\xD8\xFF` sniff `export-pdf::decode` uses),
  returning `None` for anything else. This is the format-agnostic entry the file path needs (format
  is a property of the bytes, not the caller).
- Add `ProxyCache::insert_image(id, bytes)` (sniff + insert) and
  `ProxyCache::populate_from_assets(assets, base_dir)` — a read-only pass that resolves each asset's
  path against `base_dir`, reads it, and caches its proxy under the asset **id**.

The only new dependency is the internal **`quill-core-model`** crate (for `Asset`) — an acyclic edge
(`core-model` is the base crate; `render` is already downstream of it in the documented data flow). No
new *external* crate, no serialized-model/format change.

## Behavior

### `decode_image_proxy`

```rust
/// Decode PNG or JPEG image `bytes` into a screen [`Proxy`], dispatched on the leading magic bytes
/// (mirrors `export-pdf`'s `decode`). Returns `None` for unknown/unsupported formats or any decode
/// failure — a missing screen proxy is recoverable (skip, don't panic).
pub fn decode_image_proxy(bytes: &[u8]) -> Option<Proxy>;
```

- `bytes` starts with `\x89PNG\r\n\x1a\n` → [`decode_png_proxy`].
- `bytes` starts with `\xFF\xD8\xFF` → [`decode_jpeg_proxy`].
- otherwise → `None`.

### `ProxyCache::insert_image` / `populate_from_assets`

```rust
impl ProxyCache {
    /// Sniff `bytes` (PNG/JPEG) and cache the proxy under `asset_id`; `false` (storing nothing) if
    /// undecodable/unsupported.
    pub fn insert_image(&mut self, asset_id: &str, bytes: &[u8]) -> bool;

    /// Read and cache a screen proxy for each asset, resolving `asset.path` against `base_dir`.
    /// Missing, unreadable, or unsupported files are skipped (not fatal — a broken link must not
    /// abort loading a 500-page doc). Returns the number of proxies successfully generated. A cached
    /// proxy is keyed by the asset's `id`.
    pub fn populate_from_assets(&mut self, assets: &[Asset], base_dir: &Path) -> usize;
}
```

- For each asset: `path = base_dir.join(&asset.path)`; `fs::read(path)`; on success sniff+decode+store
  under `asset.id`; on any failure (missing file, unreadable, unsupported/undecodable) skip.
- Returns the count of assets for which a proxy was cached. Re-running replaces existing entries.

## Inputs / outputs

- **Input:** a `&[Asset]` and a `base_dir: &Path` (the document's asset root); linked files are PNG or
  JPEG on disk.
- **Output:** the cache holds a `Proxy` (keyed by `asset.id`) for each successfully read+decoded
  asset; `populate_from_assets` returns the success count. Unsupported/missing assets are silently
  skipped.

## Acceptance criteria

- **Populates from real files:** given a `base_dir` containing a real PNG and a real JPEG referenced
  by two assets, `populate_from_assets` returns `2` and `get(id)` yields a `Proxy` for each.
- **Skips missing / unsupported, non-fatally:** an asset whose `path` is absent, or points at a
  non-image file, is skipped — `populate_from_assets` counts only the successes and does not panic;
  the cache is left without that id.
- **Content sniff:** `decode_image_proxy` returns `Some` for PNG bytes, `Some` for JPEG bytes, and
  `None` for non-image bytes — dispatch is by magic bytes, not by any filename extension.
- **`insert_image` round-trip:** `insert_image` then `get` returns a `Proxy` with the expected
  dimensions; `insert_image` on garbage returns `false` and stores nothing.
- Full workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`).

## Non-goals (named follow-ups)

- **On-disk (persisted) proxy cache + invalidation** — regenerate when the linked file's mtime/hash
  changes; this increment is in-memory and always regenerates.
- **CMYK / `L16` proxies** (color-managed screen conversion) — still deferred (spec 0022 non-goal).
- **Lazy / concurrent proxy generation, eviction under memory pressure, GPU upload / Skia canvas** —
  later render-track work.
- **A document-open flow that calls `populate_from_assets`** — the `render` API is provided here; the
  app/CLI wiring that supplies the `base_dir` (the extracted `.tpub` root) rides with the app layer.
