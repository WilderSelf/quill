# Plan — spec 0021: linked-image proxy pixels (PNG)

Implements `specs/0021-png-proxy-pixels.md`: make the `render` `ProxyCache` hold real downsampled
screen-proxy **pixels** (RGBA8) for PNG art, not just proxy dimensions — the core "never composite
full-res on screen" perf strategy.

## Approach (reuse existing `png` dep + in-crate area-average downsample — no new dep)

- Add `png = { workspace = true }` to `render` (already a vetted workspace dep; no new crate).
- `Proxy { width, height, rgba: Vec<u8> }` — RGBA8, `len == w*h*4`.
- `decode_png_proxy(bytes) -> Option<Proxy>`: `png` decode with `normalize_to_color8()` →
  normalize Gray/GrayA/RGB/RGBA to RGBA8 → `proxy_size(src)` target → area-average downsample
  (mean RGBA over each target cell). `None` on decode failure (screen-only, skip not panic).
- `proxy_size` policy fn + `PROXY_MAX_EDGE_PX` unchanged (now shared with `decode_png_proxy`).
- `ProxyCache` stores `Proxy`: `insert_png(id, bytes) -> bool`, `get(id) -> Option<&Proxy>`
  (replaces the dims-only `insert(id,w,h)` / `get -> Option<(u32,u32)>`).

## Files to touch

- `crates/render/Cargo.toml` — add `png` workspace dep.
- `crates/render/src/lib.rs` — `Proxy`, `decode_png_proxy`, area-average downsample, cache holds
  `Proxy`; tests.
- `specs/0021-png-proxy-pixels.md`, `specs/README.md` — the spec (done).

## Test strategy (headless; synthesize PNGs in-memory via the `png` encoder — CLAUDE.md fixture rule)

- 4096×2048 RGBA PNG → Proxy 2048×1024, `rgba.len()==2048*1024*4`.
- 8×8 PNG → native size, identity pixels (no upscale).
- Known 2×2 → 1×1 == exact per-channel mean (area-average is a real average).
- Grayscale PNG → RGBA with R==G==B, A==255 (format normalization).
- Garbage bytes → `None`; `insert_png` false, cache unchanged.
- `insert_png` then `get` round-trip returns expected dims.
- Existing `proxy_size` tests stay green (unchanged policy).
- Full workspace green (fmt, clippy -D warnings, build, test).
