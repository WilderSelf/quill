# Plan — spec 0022: linked-image proxy pixels (JPEG)

Implements `specs/0022-jpeg-proxy-pixels.md`: JPEG proxy pixels in the `render` `ProxyCache`,
parallel to spec 0021's PNG path, reusing the shared `downsample_rgba` and the existing
`jpeg-decoder` workspace dep (no new dependency).

## Approach

- Add `jpeg-decoder = { workspace = true }` to `render` (already a vetted workspace dep).
- `decode_jpeg_proxy(bytes) -> Option<Proxy>`: `jpeg-decoder` decode → normalize `L8`→(g,g,g,255),
  `RGB24`→(r,g,b,255) to RGBA8 (`Rgba8`) → shared `downsample_rgba`. `CMYK32`/`L16` → `None`
  (deferred — CMYK is the spec-0012 ambiguity minefield; screen color-management is a later brick).
  Any decode error → `None` (skip, not panic).
- `ProxyCache::insert_jpeg(id, bytes) -> bool`, parallel to `insert_png`.

## Files to touch

- `crates/render/Cargo.toml` — add `jpeg-decoder` workspace dep.
- `crates/render/src/lib.rs` — `decode_jpeg_proxy`, `decode_jpeg_rgba`, `insert_jpeg`; tests.
- `crates/render/assets/{test_gray,test_rgb,test_cmyk}.jpg` — tiny committed fixtures (copied from
  export-pdf; JPEG is decode-only so can't synthesize in-memory).
- `specs/0022-jpeg-proxy-pixels.md`, `specs/README.md` — the spec (done).

## Test strategy (committed tiny fixtures)

- RGB JPEG → Proxy at native dims, `rgba.len()==w*h*4`, all alpha 255.
- Grayscale JPEG → RGBA with R==G==B, A==255.
- CMYK JPEG → `None`; `insert_jpeg` false, cache unchanged.
- Garbage bytes → `None`.
- `insert_jpeg` then `get` round-trip returns expected dims.
- Existing PNG/downsample tests stay green (shared kernel unchanged).
- Full workspace green (fmt, clippy -D warnings, build, test).
