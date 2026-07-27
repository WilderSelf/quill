# 0033 — Screen render: paint list, CPU rasterizer, `quill render`

**Milestone:** M1 · **Status:** implemented

## Why

`quill-render` was a CPU image-decode and downsample library with a memoizing cache — no canvas, no
page traversal, no way to draw anything. **Nothing in the workspace depended on it.** The proxy
cache, described in `CLAUDE.md` as *the core perf strategy*, had never been exercised by anything
that draws.

The only `LaidOutPage` → drawing traversal in existence was the private, PDF-specific
`writer::render_page`. So the project could produce a press PDF and could not put a single pixel on
screen.

## What

### A paint list, then a rasterizer

`paint_page(&LaidOutPage, &PageGeom, &Font, &ProxyCache) -> Vec<PaintOp>`, then
`rasterize(&[PaintOp], …, scale) -> Raster`.

Two reasons for the split. The backend is deliberately swappable (`docs/roadmap.md` records
`tiny-skia` as the choice, with a GPU backend explicitly left open); emitting backend-neutral ops
first means that swap replaces one module rather than reaching into layout. And it makes screen
rendering **testable**: pixel goldens are flaky across platforms because anti-aliasing differs, so
the op list is the golden artifact and the raster is checked only for coarse invariants.

Everything is in top-left points — the space `layout-engine` produces. The PDF writer's bottom-left
flip is a PDF convention and does not appear here.

### The rasterizer

`tiny-skia`: pure Rust, permissive (BSD-3-Clause), same geometry semantics as Skia, and no native
C++ build on any leg of the three-OS matrix — which is why it was chosen over `skia-safe`. Glyphs are
filled from `quill-fonts` outlines, so no canvas text stack and no FreeType.

### CMYK → sRGB preview

`quill_color::to_srgb`. Explicitly a *preview* transform, not a soft proof: it does not go through
the OutputIntent profile, and pretending otherwise would be worse than being plainly approximate.
Press output never travels this path — the writer emits authored CMYK directly.

### `quill render`

A CLI subcommand writing a PNG, so the screen path gets the same kind of external gate export gets
from Ghostscript. Screen layout runs through the same shared font (spec 0032) the exporter uses, so
what is drawn is what the PDF would contain.

## Two deliberate asymmetries with export

Both are the same judgement applied in opposite directions, and both are worth stating because they
look like inconsistencies:

- **A missing image proxy draws nothing and does not panic.** On screen a broken link is recoverable
  and visible; refusing to draw the page around it would make a 500-page book unopenable because one
  asset moved. Export takes the opposite view — a silently dropped image reaching a print shop is
  not recoverable.
- **The trim guide is emitted as an op but not rasterized.** It is a viewport affordance, not part
  of the document, and drawing it into `quill render`'s output would make it indistinguishable from
  real content in the CI blankness check.

## Acceptance criteria

- [x] `paint_page` is deterministic — the same inputs twice produce an identical op list.
- [x] A page's ops begin with paper and a trim guide matching the media and trim boxes.
- [x] **The screen baseline equals the exporter's**, both derived from `Font::ascent_pt` (spec 0032). If they diverged, a line would sit in one place on screen and another on the page.
- [x] Text ops carry their own size, so styles (spec 0028) reach the screen and not only the PDF: a heading paints larger than body text.
- [x] Statics paint before flowed content, so master art sits behind it.
- [x] Press colour is converted for the screen: 100% K previews as black.
- [x] An image with no proxy emits nothing and does not panic; one with a proxy emits an op **at proxy resolution**, proving full-res is never composited on screen.
- [x] Raster dimensions follow the page and scale; 2× is exactly twice 1×.
- [x] The rendered page is **neither blank nor dark** — one assertion catching both "nothing was drawn" and "the paper fill is missing or the demultiply is inverted".
- [x] A corner pixel is paper white, pinning the fill and the premultiply round-trip.
- [x] Rasterizing is deterministic, and the emitted PNG decodes.
- [x] CI runs `quill render` and applies the same blank/dark check to the real PNG — **including undoing PNG row filters**, without which the check reads filter deltas rather than pixels and reports a blank page as dark. (That is exactly what my first attempt did.)

## Non-goals

- **A GPU backend.** The paint-list seam exists so one can be added without touching layout.
- **Soft proofing** against a real press profile.
- **CMYK JPEG proxies**, which still decode to `None` and so stay blank on screen — an inherited gap
  from spec 0022, not introduced here.
- Incremental *repaint*. `changed_pages` from spec 0031 says what to repaint; wiring it to a viewport
  is the app shell (spec 0034).
- Text selection, carets, hit-testing — editing affordances rather than rendering.
