# 0021 — Linked-image proxy pixels (PNG)

- **Milestone:** M1
- **Status:** in-progress (increment 1 — PNG decode + downsample to real proxy pixels)
- **Crates:** `quill-render` (owner)

## Problem

The `render` crate's `ProxyCache` scaffold computes proxy *dimensions* (`proxy_size`,
[`PROXY_MAX_EDGE_PX`] = 2048) but holds **no pixel data** — its doc-comment says "Actual pixel data
is attached once the GPU renderer lands." The core performance strategy (CLAUDE.md: "Never composite
full-res on screen — full-res is only touched at export") needs the cache to actually hold the
**downsampled screen proxy pixels**, so the on-screen renderer composites small proxies instead of
full-resolution art on a 500-page, image-heavy book.

Generating proxy pixels is independent of the (later) GPU canvas: it is a pure CPU decode +
downsample producing an RGBA8 buffer the GPU renderer will later upload as a texture. This spec
delivers that generation for **PNG** — the primary lossless art format, already decodable via the
workspace's existing `png` dependency — mirroring the project's one-format-at-a-time build-up
(PNG input spec 0010, JPEG input specs 0008/0012 landed separately). JPEG and other formats (and any
adoption of the umbrella `image` crate to normalize them) are explicit later increments.

## Architectural decision (reuse the existing `png` dep + an in-crate area-average downsample)

The approved plan's dependency table names the umbrella `image` crate for "decode + proxy
generation," but the workspace deliberately diverged — M0 image embedding uses the targeted `png` +
`jpeg-decoder` crates with an explicit "don't bloat the dep graph" note. To stay consistent with that
established minimal-dependency convention (and CLAUDE.md's non-negotiable "every dependency must be
permissive-compatible / avoid bloat"), this increment:

- **Adds no new dependency.** It reuses the existing `png` workspace dep (already vetted, permissive
  MIT/Apache) to decode, exactly as `export-pdf` does.
- **Downsamples with a small in-crate area-average filter** (no image-resize crate). For a screen
  proxy capped at 2048 px on the longest edge, box/area averaging is the standard, dependency-free
  downscale; proxy quality is not press-critical (full-res is used at export), so a heavier resampler
  is unwarranted here.

Whether to adopt the `image` umbrella crate (to normalize JPEG/CMYK/other formats in one place) is a
real dependency decision deferred to the JPEG-proxy increment, where it can be weighed on its own.

## Behavior

### `Proxy`

```rust
/// A decoded, downsampled screen proxy: RGBA8 pixels at the proxy dimensions. The GPU renderer
/// uploads `rgba` as a texture; nothing here touches full-resolution art after generation.
pub struct Proxy {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>, // width * height * 4 bytes, row-major, non-premultiplied RGBA8
}
```

### `decode_png_proxy`

```rust
/// Decode PNG `bytes` and downsample to a screen [`Proxy`] whose longest edge is at most
/// [`PROXY_MAX_EDGE_PX`]. Returns `None` on any decode failure (a missing/corrupt proxy is a
/// recoverable screen-only concern — skip, don't panic).
pub fn decode_png_proxy(bytes: &[u8]) -> Option<Proxy>;
```

- Decodes via `png` with `normalize_to_color8()` (expands indexed→RGB, strips 16-bit→8-bit), then
  normalizes any of Grayscale / GrayscaleAlpha / RGB / RGBA to **RGBA8** (gray → `(g,g,g,255)` etc.).
- Computes the target size with the existing `proxy_size(src_w, src_h)` policy, then downsamples with
  an **area-average**: each target pixel is the mean RGBA over its source cell
  (`[tx*sw/tw .. (tx+1)*sw/tw) × [ty*sh/th .. (ty+1)*sh/th)`). `proxy_size` never upscales, so every
  cell covers ≥ 1 source pixel; when no downscale is needed (`target == source`) the result is the
  decoded image unchanged.

### `ProxyCache` holds pixels

`ProxyCache` now stores `Proxy` values (not bare dimensions):

- `insert_png(&mut self, asset_id: &str, bytes: &[u8]) -> bool` — decode + downsample + store;
  returns `false` (and stores nothing) if the PNG can't be decoded.
- `get(&self, asset_id: &str) -> Option<&Proxy>` — the cached proxy, if present.

The pure `proxy_size` sizing policy and its constant are unchanged (now also used by
`decode_png_proxy`).

## Inputs / outputs

- **Input:** PNG bytes (grayscale / gray-alpha / RGB / RGBA / indexed, any bit depth `png`
  normalizes to 8-bit).
- **Output:** a `Proxy` with `rgba.len() == width * height * 4`, `width`/`height` ==
  `proxy_size(src_w, src_h)`, longest edge ≤ 2048.

## Acceptance criteria

- **Real downsampled pixels:** decoding a synthesized 4096×2048 RGBA PNG yields a `Proxy` of
  2048×1024 with `rgba.len() == 2048*1024*4`.
- **No upscale:** a small (e.g. 8×8) PNG yields a `Proxy` at its native size with the correct pixel
  count; pixels are preserved (identity when no downscale).
- **Area-average is a real mean:** a 2×2 image with known distinct pixel values downsampled to 1×1
  yields the exact per-channel average.
- **Format normalization:** a grayscale PNG decodes to RGBA with `R==G==B` and `A==255`.
- **Corrupt input is skipped, not fatal:** `decode_png_proxy(garbage)` returns `None`;
  `insert_png` returns `false` and leaves the cache unchanged.
- **Cache round-trip:** `insert_png` then `get` returns a `Proxy` with the expected dimensions.
- Full workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`).

## Non-goals (named follow-ups)

- **JPEG / other-format proxies**, and the decision whether to adopt the umbrella `image` crate to
  normalize them — a later increment.
- **On-disk proxy cache + invalidation** (regenerate when the linked file changes); this increment is
  in-memory only.
- **GPU texture upload / Skia canvas** — the proxy `rgba` is CPU bytes the later GPU renderer consumes.
- **Higher-quality resampling** (Lanczos/triangle), color-managed proxy generation, and proxies for
  non-`Asset` sources — later work if screen quality warrants it.
