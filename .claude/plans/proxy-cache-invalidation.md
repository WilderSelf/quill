# Plan — spec 0024: incremental proxy-cache invalidation

## Goal (one atomic increment)

Make `ProxyCache::populate_from_assets` **invalidation-aware**: skip re-decoding an asset whose
linked source file is unchanged since it was last cached, and (re)decode only new/changed files.
This is the *invalidation* half of spec 0023's named follow-up "on-disk (persisted) proxy cache +
invalidation" — done **in-memory first** (source signature = cheap `mtime + size`, no full-file
hash, no on-disk persistence, which stays deferred).

Serves the incremental-performance north star: on a 500-page book, re-running `populate_from_assets`
after an edit must not re-decode every linked image.

## Files to touch

- `crates/render/src/lib.rs` — the only code change:
  - Store proxies as an internal `CacheEntry { proxy, sig: Option<SourceSig> }` instead of a bare
    `Proxy`; `SourceSig { mtime: SystemTime, len: u64 }`.
  - `insert_png`/`insert_jpeg`/`insert_image` (byte-fed, no path) store `sig: None`.
  - `populate_from_assets` returns a `PopulateReport { generated, reused, skipped }` (was `usize`).
    For each asset: stat the file → `SourceSig`; if an entry with a byte-identical sig is cached →
    `reused += 1`, skip decode; else read+decode → store with new sig (`generated += 1`); missing/
    unreadable/unsupported → `skipped += 1` (non-destructive: any prior proxy is left intact).
  - `get` unwraps through `CacheEntry`.
  - New `PopulateReport` + private `SourceSig`/`CacheEntry`/`source_sig()`.
- `specs/0024-proxy-cache-invalidation.md` — new spec (step 0).
- `specs/README.md` — add the 0024 row (`in-progress`).

No new dependency (`std::fs::metadata` + `std::time::SystemTime`). No `.tpub`/format change.

## Test strategy (in-crate)

- **Unchanged → reused:** populate twice over the same dir, no file touched → 2nd call
  `reused == N, generated == 0`; `get` still yields the proxy.
- **Changed → regenerated:** overwrite one file with a **different-length** image (size change makes
  the sig differ deterministically, independent of mtime granularity) → 2nd call `generated == 1`
  for it, and `get` reflects the new dimensions; the untouched sibling is `reused`.
- **New asset between calls** is generated; siblings reused.
- **Missing/unsupported** counted in `skipped`, non-fatal; a previously-cached proxy for a
  now-missing file is left intact.
- Update the two existing `populate_from_assets` tests to assert on `.generated`.

## Non-goals (still deferred)

- On-disk persistence of the cache across process restarts.
- Content-hash invalidation (mtime+size can miss a same-size, same-mtime edit — the documented
  heuristic limit; hashing is the heavier follow-up).
- Eviction under memory pressure, lazy/concurrent generation, GPU upload / Skia canvas.
- CMYK / `L16` color-managed proxies.
