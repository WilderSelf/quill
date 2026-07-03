# Plan — spec 0023: populate the proxy cache from a document's linked assets

Implements `specs/0023-proxy-cache-from-assets.md`: wire the `render` `ProxyCache` to a `Document`'s
`Asset` list — content-sniff each linked file and cache its proxy. Mirrors `export-pdf`'s
`images::resolve`/`decode` front-end.

## Approach

- Add `quill-core-model = { workspace = true }` to `render` (internal, acyclic — for `Asset`).
- `decode_image_proxy(bytes) -> Option<Proxy>`: magic-byte sniff (`\x89PNG…` → `decode_png_proxy`,
  `\xFF\xD8\xFF` → `decode_jpeg_proxy`, else `None`) — same sniff as `export-pdf::decode`.
- `ProxyCache::insert_image(id, bytes) -> bool`: sniff + insert.
- `ProxyCache::populate_from_assets(assets: &[Asset], base_dir: &Path) -> usize`: for each asset,
  `fs::read(base_dir.join(&asset.path))` → `insert_image(&asset.id, &bytes)`; count successes; skip
  missing/unreadable/unsupported (non-fatal). `use std::path::Path`.

## Files to touch

- `crates/render/Cargo.toml` — add `quill-core-model` workspace dep.
- `crates/render/src/lib.rs` — `decode_image_proxy`, `insert_image`, `populate_from_assets`; tests.
- `specs/0023-proxy-cache-from-assets.md`, `specs/README.md` — the spec (done).

## Test strategy (headless; temp dir + real fixture bytes, per export-pdf's temp-file test idiom)

- `populate_from_assets`: write a real PNG (synthesized via `png` encoder) + copy a fixture JPEG to a
  unique temp dir; two assets referencing them → returns 2, `get(id)` yields a Proxy for each.
- Missing path + a non-image file → skipped, counted only on success, no panic, cache lacks those ids.
- `decode_image_proxy`: PNG bytes → Some, JPEG bytes → Some, garbage → None (magic-byte dispatch).
- `insert_image` round-trip + garbage → false.
- Existing PNG/JPEG/downsample tests stay green.
- Full workspace green (fmt, clippy -D warnings, build, test).
