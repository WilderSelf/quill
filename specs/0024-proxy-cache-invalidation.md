# 0024 — Incremental proxy-cache invalidation (skip unchanged assets)

- **Milestone:** M1
- **Status:** in-progress (increment 1 — in-memory mtime+size invalidation)
- **Crates:** `quill-render` (owner)

## Problem

Spec 0023 gave `ProxyCache` a `populate_from_assets(assets, base_dir)` pass that reads each linked
file and caches a downsampled screen proxy. But it **always re-decodes every asset on every call**:
its own doc-comment says "Re-running replaces existing entries." On a 500-page, image-heavy book that
means every re-populate (after any edit that re-runs the pass) decodes + downsamples *hundreds* of
full-resolution images — the opposite of the incremental-performance guarantee that is the whole
reason proxies exist ("editing one thing must not re-do the whole document").

The proxy cache must **invalidate incrementally**: reuse a cached proxy when its linked source file
is unchanged, and (re)decode only the assets that are new or whose source changed.

## Architectural decision (in-memory `mtime + size` signature; no new dep, no on-disk format)

This is the *invalidation* half of spec 0023's named follow-up "on-disk (persisted) proxy cache +
invalidation." It is done **in-memory first**, matching the project's policy-before-persistence
pattern (proxy *sizing* landed before proxy *pixels*):

- Each cached proxy carries a **source signature** derived from `std::fs::metadata`:
  `SourceSig { mtime: SystemTime, len: u64 }`. `metadata` is a cheap `stat` — it never reads or
  hashes the file body, so re-populating a 500-page doc is hundreds of `stat`s, not hundreds of
  decodes.
- `populate_from_assets` reuses a cached proxy when the asset's id is already cached with a
  **byte-identical** signature; otherwise it reads + decodes and records the new signature.

No new dependency (`std::fs::metadata` + `std::time::SystemTime` are std). No serialized-model or
`.tpub` format change — the signature lives only in the in-memory cache.

**Heuristic limit (deliberate):** `mtime + size` misses an in-place edit that preserves both byte
length *and* modification time. Content-hash invalidation (which would catch it) is a named,
heavier follow-up; this increment takes the cheap, standard signature that build tools use.

## Behavior

```rust
/// Outcome counts for one [`ProxyCache::populate_from_assets`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PopulateReport {
    /// Assets decoded fresh this call — new, or their source file changed since last cached.
    pub generated: usize,
    /// Assets whose cached proxy was reused because the source file was unchanged (no decode).
    pub reused: usize,
    /// Assets with no proxy this call — missing, unreadable, or unsupported/undecodable. Any proxy
    /// cached for that id on a prior call is left intact (a vanished link shows its last-known art).
    pub skipped: usize,
}

impl ProxyCache {
    /// Read and cache a screen proxy for each asset, resolving `asset.path` against `base_dir`,
    /// **skipping the decode for any asset whose source file is unchanged** since it was last cached
    /// (by `mtime + size`). Each proxy is keyed by the asset's `id`.
    ///
    /// Missing, unreadable, or unsupported files are skipped, not fatal — a broken link must not
    /// abort loading a 500-page document, and does not evict a proxy cached on a prior call.
    pub fn populate_from_assets(&mut self, assets: &[Asset], base_dir: &Path) -> PopulateReport;
}
```

Per asset (`path = base_dir.join(&asset.path)`):

- **Reuse:** the id is already cached, the file's current signature reads successfully, and it equals
  the stored signature → count `reused`, do **not** read or decode the file.
- **Generate:** no cached entry, no readable signature, or the signature differs → `fs::read` +
  `decode_image_proxy` (content-sniff PNG/JPEG); on success store `{ proxy, sig }` under the id and
  count `generated`.
- **Skip:** the file is missing/unreadable, or decodes to nothing (unsupported/corrupt) → count
  `skipped`; any previously cached proxy for that id is left untouched.

The byte-fed inserts (`insert_png` / `insert_jpeg` / `insert_image`) have no source path, so they
store no signature — a later `populate_from_assets` for the same id will therefore regenerate from
the file (its signature differs from "none"). `get(id)` is unchanged: `Option<&Proxy>`.

## Inputs / outputs

- **Input:** a `&[Asset]` and `base_dir: &Path`; linked files are PNG/JPEG on disk. The cache may
  already hold proxies + signatures from a prior call.
- **Output:** the cache holds a `Proxy` (keyed by `asset.id`) for each currently-decodable asset;
  `populate_from_assets` returns a `PopulateReport` partitioning the assets into generated / reused /
  skipped.

## Acceptance criteria

- **Unchanged source is reused, not re-decoded:** calling `populate_from_assets` twice over the same
  unchanged directory reports `reused == N, generated == 0` on the second call, and `get(id)` still
  yields each proxy.
- **Changed source is regenerated:** after a first pass, overwriting one linked file with an image of
  **different pixel dimensions** (hence different byte length → signature differs) makes the next
  pass report `generated == 1` for it and `get(id)` reflect the new dimensions; an untouched sibling
  is `reused` in the same pass.
- **New asset between calls is generated;** assets carried over unchanged are `reused`.
- **Missing / unsupported are skipped non-fatally** and counted in `skipped`; a proxy cached for an
  id on a prior pass is **not** evicted when that file later goes missing.
- Full workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`).

## Non-goals (named follow-ups)

- **On-disk (persisted) proxy cache** — surviving process restarts; this increment's signature lives
  only in memory. (The other half of spec 0023's invalidation follow-up.)
- **Content-hash invalidation** — catching a same-size, same-mtime in-place edit that the `mtime +
  size` heuristic misses.
- **Eviction under memory pressure, lazy / concurrent proxy generation, GPU upload / Skia canvas** —
  later render-track work.
- **CMYK / `L16` color-managed proxies** — still deferred (spec 0022 non-goal).
