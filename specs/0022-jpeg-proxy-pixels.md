# 0022 — Linked-image proxy pixels (JPEG)

- **Milestone:** M1
- **Status:** in-progress (increment 1 — baseline grayscale + RGB JPEG proxies)
- **Crates:** `quill-render` (owner)

## Problem

Spec 0021 gave the `render` `ProxyCache` real downsampled screen-proxy pixels for **PNG** art. TTRPG
source art is just as often **JPEG** (photographic textures, scanned maps). The proxy cache must
generate proxies for JPEG linked images too, or those assets have no screen proxy and would force
full-resolution compositing — exactly what the proxy strategy exists to avoid.

The decode + downsample machinery is already in place (spec 0021): `Proxy`, the shared area-average
`downsample_rgba`, and the `proxy_size` policy. This spec adds a JPEG decode front-end that
normalizes to the same RGBA8 buffer, reusing all of it.

## Architectural decision (reuse the existing `jpeg-decoder` dep — still no `image` crate)

Consistent with spec 0021's reasoning: the approved plan's dependency table names the umbrella `image`
crate, but the workspace uses targeted decoders. JPEG is already decodable via the **existing
`jpeg-decoder` workspace dependency** (`export-pdf` uses it), so this increment reuses it — **no new
dependency, no `image` crate**. The shared `downsample_rgba` from spec 0021 is format-agnostic and is
reused unchanged.

## Behavior

### `decode_jpeg_proxy`

```rust
/// Decode baseline/progressive JPEG `bytes` and downsample to a screen [`Proxy`]. Returns `None` on
/// any decode failure, and for pixel formats this increment does not handle (see below) — a missing
/// screen proxy is recoverable (skip, don't panic).
pub fn decode_jpeg_proxy(bytes: &[u8]) -> Option<Proxy>;
```

Normalizes `jpeg-decoder`'s output pixel formats to RGBA8, then downsamples with the shared
`downsample_rgba`:

- **`L8`** (8-bit grayscale) → `(g, g, g, 255)`.
- **`RGB24`** → `(r, g, b, 255)`.
- **`CMYK32`** and **`L16`** → **skipped (`None`)** this increment. A CMYK JPEG is the same ambiguity
  minefield spec 0012 documents (Adobe transform / YCCK inversion — `jpeg-decoder` yields YCCK as
  `[R,G,B,255-K]`, unusable as CMYK); rendering an approximate or wrong-color screen proxy from it is
  deferred to a later color-managed proxy increment. `L16` (16-bit gray) is likewise deferred.

### `ProxyCache::insert_jpeg`

```rust
/// Decode + downsample JPEG `bytes` and cache the resulting Proxy under `asset_id`. Returns `false`
/// (storing nothing) if the JPEG can't be decoded or is an unhandled pixel format.
pub fn insert_jpeg(&mut self, asset_id: &str, bytes: &[u8]) -> bool;
```

Parallel to spec 0021's `insert_png`. (A format-sniffing unified `insert_image` is a later
convenience; callers currently know an asset's format from its path/type.)

## Inputs / outputs

- **Input:** baseline/progressive JPEG bytes (grayscale `L8` or `RGB24`).
- **Output:** a `Proxy` with `rgba.len() == width * height * 4`, dimensions == `proxy_size(src)`,
  longest edge ≤ [`PROXY_MAX_EDGE_PX`]. `None` for CMYK/`L16`/undecodable input.

## Acceptance criteria

- **RGB JPEG → RGBA proxy:** decoding an RGB JPEG yields a `Proxy` at the source dimensions (≤ 2048)
  with `rgba.len() == w*h*4` and every alpha byte `255`.
- **Grayscale JPEG → opaque gray RGBA:** an `L8` JPEG decodes to RGBA with `R==G==B` and `A==255`.
- **CMYK JPEG is skipped:** `decode_jpeg_proxy` on a CMYK JPEG returns `None` (deferred, not
  wrong-color); `insert_jpeg` returns `false` and leaves the cache unchanged.
- **Corrupt input is skipped, not fatal:** `decode_jpeg_proxy(garbage)` returns `None`.
- **Cache round-trip:** `insert_jpeg` then `get` returns a `Proxy` with the expected dimensions.
- **Shared downsample:** a JPEG larger than 2048 on its longest edge is downsampled to `proxy_size`
  (reusing spec 0021's `downsample_rgba`, verified by dimensions). *(JPEG is lossy + decode-only, so
  large-image coverage is exercised via the PNG path in spec 0021; the JPEG tests use the committed
  tiny fixtures and assert format normalization + skip behavior.)*
- Full workspace green (`fmt`, `clippy --all-targets --all-features -D warnings`, `build`, `test`).

## Test fixtures

Reuses the tiny committed 8×8 JPEG fixtures (`test_gray.jpg`, `test_rgb.jpg`, `test_cmyk.jpg`) —
JPEG is lossy and `jpeg-decoder` is decode-only, so (unlike PNG) fixtures are committed rather than
synthesized in-memory (CLAUDE.md fixture guidance). Copied into `crates/render/assets/` so the crate
is self-contained.

## Non-goals (named follow-ups)

- **CMYK / `L16` JPEG proxies** — need a color-managed CMYK→RGB screen conversion (and the spec-0012
  Adobe-marker disambiguation); a later increment.
- **A unified format-sniffing `insert_image`**, on-disk proxy cache + invalidation, GPU texture
  upload / Skia canvas, higher-quality resampling — later work (as in spec 0021).
