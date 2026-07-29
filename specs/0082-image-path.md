# 0082 — What the image path gets wrong

**Milestone:** M7 · **Status:** implemented

## Why

Two defects, both shipping today, both in the image path — the part of the workspace that has had
the least attention since M0. Like spec 0081 this is press-correctness rather than a feature, and it
runs before every M7 feature for 0075's reason: a live defect outranks a new increment. It is also
the path the rest of M7 builds on, which is the second argument for its position — every fit mode,
crop and transform makes more of the page geometry a function of asset metadata, and the
invalidation defect below is the path all of it flows through.

**(D) Image alpha was discarded rather than flattened, and the warning said the opposite.** Preflight
promised the author that the asset *"will be flattened to opaque for PDF/X"*. What actually happened
was that the alpha channel was **dropped** and the RGB stored *underneath* it converted — no
composite, no matte, no `/SMask`:

```rust
ColorType::Rgba => {
    let rgb: Vec<u8> = data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    Pixels::Cmyk(cmyk.convert(&rgb))
}
ColorType::GrayscaleAlpha => Pixels::Gray(data.chunks_exact(2).map(|p| p[0]).collect()),
```

**PNG places no constraint whatsoever on the colour stored under `alpha = 0`**, and most encoders
write `(0,0,0,0)`. Under the naive conversion that is `k = 1 − max(r,g,b) = 1`: **solid black**. So a
logo with a transparent surround exported as a black rectangle, in a press file, under a *Warning*
asserting that the opposite had happened. The pixels were legal CMYK and under 240%, so nothing
downstream could catch it — the failure was silent by construction, which is the shape `CLAUDE.md`
ranks worst.

Three specs *describe* the behaviour as "alpha is dropped" (0005 req 4, 0007, 0010), so it was
deliberate. But **no spec stated the resulting colour**, no test asserted a single emitted sample,
and the string promised compositing. The behaviour was decided; its consequence was not.

**(C) Editing an `Asset` invalidated nothing, so a relinked image served stale pages.** `image_size`
reads `px_w`/`px_h`/`dpi`, and those set the placed height and therefore every page break after it.
But `content_fingerprint`'s image arm hashed only the id string:

```rust
Block::Image { asset, .. } => { eat(b"i"); eat(asset.as_bytes()); }
```

`doc.assets` was absent from `context_fingerprint`'s argument list, and `doc.revision` is never
consulted. So correcting a dpi, or relinking to a differently-shaped file, read as "nothing changed"
and `relayout` returned the previous pages verbatim. The three session tests that touch assets all
*clear* them; nothing exercised mutation.

## The alpha decision

**Composite over paper white.** Of the two honest resolutions — composite, or say plainly what is
dropped and change the warning text — this one is right, and four arguments say so. They are given
in ascending order of force.

1. **The message already promises it.** An interface that says "flattened to opaque" and then
   discards is wrong in one of two ways, and the cheaper repair is the one that makes a true
   sentence true rather than the one that teaches an author to expect less.

2. **"Flatten" has an established meaning**, and it is not this one. In every DTP application an
   author arrives from, flattening composites the layers onto the backdrop and yields an opaque
   result; discarding an alpha channel is what a *converter* does, not what a publisher does. A
   general-purpose desktop publishing application that says "flatten" and means "discard" is using
   the word against its trade.

3. **Dropping alpha is the behaviour that produces the black rectangle**, and it is silent. Spec
   0006 makes an over-inked image pixel structurally impossible; spec 0081 makes an illegal *colour*
   a refusal. Neither can see this, because a flattened-wrong pixel is a legal colour within the ink
   limit. `CLAUDE.md`'s rule — *prefer a visible failure over silent press-corruption* — has no
   third option to offer here: there is no "skip it loudly" available, because the image decodes
   perfectly and the only question is what its transparent region means. When the choice is between
   two answers rather than between an answer and a refusal, the rule reduces to picking the one that
   is not silently wrong.

4. **The screen already composites, so this is the choice that removes a divergence rather than
   creating one.** `rasterize` fills the page with `PAPER` and draws the proxy `SrcOver` on top, so a
   transparent pixel has always *displayed* as paper. Under (D) the same pixel printed as solid
   black. That is exactly the drift class `CLAUDE.md`'s one-shaper rule exists to prevent — screen
   and press disagreeing about what the author will get — and it is the decisive argument, because
   the alternative resolution does not merely leave the divergence in place, it **ratifies** it.

### What compositing costs, measured

Per-pixel work on full-resolution art at export, which is real and is stated rather than waved at.
It is bounded three ways:

- **Only alpha-bearing images pay it.** `Grayscale`, `Rgb` and every JPEG path are untouched, and
  `flatten_sample_over_paper(s, 255) == s` byte-identically, so even an opaque RGBA image emits the
  bytes it always did.
- **It adds no traversal.** The `Rgba` arm already walked the buffer to widen 4 channels down to 3;
  it now does arithmetic in that walk instead of discarding a byte. Same allocation, same pass.
- **Measured, on a 12 MP RGBA image**: the composite takes **41.8 ms** against the **18.8 ms** widen
  it replaces — **+23 ms** — feeding a naive CMYK conversion that takes **210.6 ms** on the same
  buffer. So it is ~11% of the conversion step it precedes, and a smaller fraction still of the PNG
  decode and the deflate that bracket it.

### Composite, then convert, then clamp — and why that order

The order is forced, not chosen, and each step is forced by a different thing.

**Composite before convert**, because alpha means "fraction of the *source* colour" and is only
defined in the space the source states its colour in. Compositing after conversion would be a blend
of *ink amounts* against a paper of `(0,0,0,0)`, which is a different operation with a different
result and is not what the author saw on screen. It also has to happen in the source space for the
screen path to be able to share the code at all — the proxy never becomes CMYK.

**Clamp after both**, because spec 0006's clamp is a guarantee about the pixels that are *embedded*.
Clamping a pixel the composite then moves would guarantee a number that never reaches the file. The
concrete gain from putting the composite *upstream* is that spec 0006's chokepoint argument survives
untouched: `RgbToCmyk::convert` is still the single site where the ≤240% limit is applied, and every
pixel that reaches it is already opaque, so "an over-ink image pixel is structurally impossible"
remains true for exactly the reason 0006 gave. Had the composite been folded into or after `convert`,
the clamp would have needed a second site and the guarantee a second proof.

Note what is *not* claimed: compositing toward white is not proven to reduce ink monotonically under
an arbitrary ICC transform, so the clamp is not redundant and still runs on every pixel.

### One definition of what a transparent pixel becomes

`quill-color` owns it, because both crates already depend on it and the alternative is two
implementations of one rule:

```rust
pub const PAPER_SAMPLE: u8 = 255;
pub fn flatten_sample_over_paper(sample: u8, alpha: u8) -> u8;
pub fn flatten_over_paper(src: &[u8], channels: usize) -> Vec<u8>;
```

`channels = 1` serves `GrayscaleAlpha`, `channels = 3` serves `Rgba`, and **`tRNS` needs no arm of
its own** — spec 0010's `EXPAND` has already turned a keyed palette into `Rgba` and a keyed grey
level into `GrayscaleAlpha` before either decoder sees it, which is why covering the two arms covers
all three inputs. That was verified against `png`'s `output_color_type`, not assumed.

The arithmetic is integer and exactly round-half-up (`(v + 127) / 255`, exact for every product two
`u8`s can make), so screen and press cannot disagree by a rounding mode — the failure that would
otherwise turn one shared rule back into two.

`render`'s `paint::PAPER` — the colour a page is *filled* with — is now built from `PAPER_SAMPLE`
rather than spelled out as `[255, 255, 255]`. Two constants that agree today is exactly how screen
and press begin to drift, and this one is the backdrop of both operations.

### The screen path takes it too, at the same point

`decode_png_rgba` in `render` composites at decode, **before** `downsample_rgba`, and the proxy is
opaque from then on. Three consequences, all wanted:

- **Screen and press agree by construction**, not by the page underneath happening to be white. A
  proxy drawn over a filled decoration used to blend against *that*; the press has only ever had one
  backdrop available to it, and now so does the screen.
- **A fringe defect the screen carried independently is fixed.** `downsample_rgba` averages
  *non-premultiplied* RGBA, so the black stored under `alpha = 0` was averaged into its neighbours: a
  red logo on a `(0,0,0,0)` surround acquired a dark halo at every proxy edge. Compositing first
  removes it, and `downsampling_a_transparent_edge_does_not_darken_it` pins it.
- **`Proxy::rgba` is now always opaque**, and its doc comment says so. This is an honest loss and is
  recorded as one: the app can no longer show live transparency on screen. It could not print it
  either — PDF/X-1a and X-3 forbid it — so what was lost is the ability to preview something the
  product cannot produce. The alpha byte stays in the layout so the texture format and the blitter
  are unchanged and spec 0089's PDF/X-4 path has somewhere to put a real one.

### The decoder reports what it found

`preflight`'s `Transparency` warning fired **only when the author had set `has_alpha` by hand**,
while the flattening happens whenever the pixels say so. Since this increment the warning asks the
file:

```rust
let carries_alpha = images::probe_alpha_at(asset, &opts.asset_root).unwrap_or(asset.has_alpha);
```

`probe_alpha_at` reads the **header only** — `IHDR`/`PLTE`/`tRNS` all precede `IDAT` by PNG's own
ordering rules, so no image data is decoded — and asks the question through
`Decoder::output_color_type()` under *the same transformations the decode uses*. That is what makes
the answer "does this file take a path that composites?" rather than "does the `IHDR` colour type
happen to end in Alpha?", and it is why a `tRNS`-keyed palette answers `true`.

The thing that knows is the decoder, because it is the thing that does the compositing. Where the
link does not resolve, `probe_alpha_at` answers **`None` — "don't know", which is a third answer and
not a `false`** — and the declaration is all there is, so a preflight run before the art is in place
still reports what the author declared.

**The warning text keeps its pre-0082 sentence byte for byte** and gains a parenthetical:

> `asset 'x' has an alpha channel; it will be flattened to opaque for PDF/X (transparent areas
> composite onto white paper)`

The old sentence is now *true*, which is a property of having chosen compositing and is worth
noticing: the resolution that required no interface churn is the one that made the existing promise
correct. The clause is added because the backdrop is information an author needs — a logo drawn to
sit on a dark ground will not survive this and should be told so — and it is appended rather than
substituted because a message an author has learned to search for is part of the interface
(spec 0081).

**`px_w`/`px_h`/`dpi` are deliberately still author-declared, and it is *not* the same fix.** The
same `read_info()` call yields the pixel dimensions, so the mechanics coincide — but the field the
press check actually gates on is **`dpi`, which is not in a PNG header at all** (`pHYs` is optional
and usually absent). Probing dimensions alone would move placed geometry — spec 0009 records
auto-measurement as a non-goal precisely because it does — while leaving the layout-critical field
declared and unchecked. Half a probe that changes where the ink lands is worse than none. A
preflight check that *reports* a declared size the file contradicts is a real follow-up and is named
in Non-goals; it is a new `CheckId`, not this increment.

## The asset-invalidation decision

**`content_fingerprint`'s image arm, not `context_fingerprint`** — and the argument is against both
named precedents, because they point in opposite directions.

**Against 0078 (whole-document).** The index took the `context_fingerprint` route on two grounds and
this change matches neither. First, *arithmetic*: **there is one index block**, as there is one
contents list, so a changed derivation costs one whole-document reflow. An art-heavy 500-page book —
the document this product exists for — has **hundreds of assets**. That is the company a
cross-reference keeps, not the company a master page keeps, and `context_fingerprint` sets
`dirty_from = Some(0)`: correcting one plate's dpi would reflow all 500 pages. Second, *shape*: 0078
also argued the index could not be a `MeasureKey` term because such a term must be a property of the
block being measured, and an index's entries derive from marks scattered through every other block.
An asset's dimensions are exactly a property of the block that places it — `image_size` is called on
one asset for one block and reads nothing else.

**With 0076 (per-block), and one step further.** A resolved cross-reference goes in `MeasureKey`
because a book has hundreds of them and a whole-document reflow per keystroke is not a cost anyone
would accept. Same arithmetic, same answer. But 0076's *sibling* — a footnote's text — is XOR'd into
the per-block diff and kept **out** of `MeasureKey`, because a note changes where a paragraph fits
and not what it measures. **An asset's metadata is the other kind**, and this is the distinction the
fix turns on: `image_size` *is* the measurement. So it belongs in `content_fingerprint` itself, which
is a `MeasureKey` field — a term that only dirtied the block would mark it dirty and then let
`CachingMeasurer` serve the stale height straight back out of the cache.

`content_fingerprint` therefore takes the `AssetIndex` as a second argument. Both call sites — the
per-pass diff and the measurement key — read the same index, so the diff and the key cannot disagree
about what a relinked id resolves to.

### Why `Debug` over the whole `Asset`

Rather than the three fields layout reads today, for `context_fingerprint`'s stated reason and with
M7 immediately ahead: **a hand-written fingerprint silently stops covering a field the moment one is
added**, and it shows up as a document that refuses to re-flow after an edit nobody can see. The
roadmap's own case for (C) is that fit modes, crops and transforms are about to make *more* of the
placed geometry a function of asset metadata. The cost of also covering `line_art` and `has_alpha`,
which layout does not read, is one re-measure of one image block when a preflight-only flag is
corrected — returning the identical height. That is the trade `context_fingerprint` already made.

An id that resolves to no asset hashes a distinguished marker rather than nothing, so *adding* the
missing asset is itself a change: until it exists the block is placed as spec 0009's square
placeholder, and that placeholder has a height too.

## `FORMAT_VERSION`

**Stays 10**, and this was checked rather than assumed. The bump rule turns on a *silence* — an older
build reading a newer document and being quietly wrong about it. This increment adds **no model
surface at all**: no field, no variant, no serialized key anywhere in `Document`, `Asset`,
`StyleSheet` or `PageTemplate`. `Asset` is untouched, including `has_alpha`, which is now a
*fallback* for an unresolvable link rather than the authority — a change in how a field is read, not
in what is written. `flatten_over_paper` is a pure function over pixels and `probe_alpha_at` reads a
linked file; neither is persisted. There is no document an older build could read differently.

`TEMPLATE_VERSION` stays **1**, `PACK_VERSION` stays **1** and `BOOK_VERSION` stays **2**, by the
same argument.

What *does* change for an older build is the **bytes it would emit for the same document** — an
alpha-bearing image now embeds different samples. That is the point of the increment and is not a
format concern: it is the same file being exported correctly instead of incorrectly, and the older
build's output was the defect.

## Digests and budgets

**`SAMPLE_EXPORT_DIGEST` did not move** (`0x67f4_3eff_e797_f120`), verified rather than assumed:
`export_of_the_sample_document_is_byte_stable` passes untouched. That is the expected result and it
is a real check. `Document::sample()` carries an asset record (`map1.png`) but no `Block::Image`, so
nothing is placed and nothing is embedded — and the file it names does not exist anywhere in the
repository, so `probe_alpha_at` answers `None` for it and the fallback reproduces the pre-0082
`Transparency` behaviour exactly. `quill preflight`'s golden report over the sample
(`crates/cli/tests/golden/preflight-sample.txt`) is byte-identical for the same reason.

**No budget in `benches/budgets.toml` moved**, and `cargo bench -p quill-testdoc` reports *all
budgets met* across all four benches. Two entries were checked specifically:

- `proxy_cache.ms_per_image` measured **2.935** against a budget of 7.0. The bench's synthesized
  fixture is **RGB**, so it takes the arm this increment does not touch. Stated plainly rather than
  presented as a pass: the benchmark does not cover the alpha path, so it could not have moved.
- `export.sample_bytes` (8454) and `export.synthetic_500_page_bytes` (1308263) are unchanged, which
  follows from the sample and the synthetic document placing no alpha-bearing image.

No entry was added. `budgets.toml` says what it is for — *blowup detectors*, guarding the **shape**
of the algorithms — and the composite adds arithmetic inside a traversal that already existed rather
than a new pass, a new allocation or a new asymptotic. The measured 12 MP figures above are the
honest statement of its cost; a budget line would be measuring a constant factor the file explicitly
declines to measure.

## Acceptance criteria

- [x] A PNG storing `(0,0,0,0)` under a transparent region exports as **no ink** (`[0,0,0,0]`), not
      solid K — asserted on the samples read back out of the finished PDF's image XObject, not on
      "it exported".
- [x] `GrayscaleAlpha` flattens to paper white on the `/DeviceGray` path.
- [x] Both `tRNS` routes are covered: a keyed **palette** (→ `Rgba`) and a keyed **grey level**
      (→ `GrayscaleAlpha`).
- [x] Partial alpha composites toward paper rather than snapping to either endpoint.
- [x] Flattened pixels are still ≤240% ink (composite first, clamp last).
- [x] An image with **no** alpha channel decodes byte-identically, on both the CMYK and gray paths.
- [x] The screen proxy is paper white and opaque for a transparent pixel, on both PNG arms, and a
      downsampled transparent edge does not darken.
- [x] **Screen and press agree** about a transparent pixel *and* about an opaque one, compared
      end-to-end through `lay_out_for_screen` → `paint_page` → `rasterize` against the real exported
      PDF.
- [x] The `Transparency` warning fires for an **undeclared** alpha channel and stays silent for a
      declaration the file contradicts; an unresolvable link falls back to the declaration.
- [x] Correcting an asset's `dpi` re-lays the pages it affects, and the session lands on the pages
      the cold path produces. Same for relinking to a differently-shaped file.
- [x] A relayout with no asset edit re-measures **0** blocks and reuses every page.
- [x] Editing the *later* of two assets reuses the pages before it (the per-block claim).
- [x] `SAMPLE_EXPORT_DIGEST` unmoved; `FORMAT_VERSION` unmoved at 10; no budget moved.
- [x] Full workspace validate green: fmt, clippy `-D warnings`, build, test, bench.

## Test strategy

**Every test below was written against the defects as they shipped and run before the fix**, which is
a stronger proof than a reintroduction and was possible here because both defects were real in the
tree. Measured on `33e8325` with only the test code added: **14 failed, 4 passed**, and the four that
passed are the ones that must pass on both sides.

| Test | Where | Against the shipped defect |
|---|---|---|
| `a_fully_transparent_rgba_pixel_is_paper_not_black` | `images.rs` | fails — `[0, 0, 0, 255]`, solid K |
| `a_fully_transparent_grayscale_alpha_pixel_is_paper_white` | `images.rs` | fails — `[0, 0]` |
| `a_trns_keyed_indexed_png_flattens_onto_paper` | `images.rs` | fails — `[0, 0, 0, 255]` |
| `a_trns_keyed_grayscale_png_flattens_onto_paper` | `images.rs` | fails — `[0, 128]` |
| `partial_alpha_composites_toward_paper` | `images.rs` | fails — `[0, 0, 0, 255]` |
| `a_transparent_logo_does_not_export_as_a_black_rectangle` | `lib.rs` | fails — **the press file's own bytes**, `[0,0,0,255]` |
| `the_transparency_warning_reports_the_file_not_the_declaration` | `lib.rs` | fails — no finding at all |
| `a_transparent_proxy_pixel_is_paper_white_and_opaque` | `render` | fails |
| `a_transparent_grayscale_alpha_proxy_pixel_is_paper_white` | `render` | fails |
| `downsampling_a_transparent_edge_does_not_darken_it` | `render` | fails — the screen's own fringe defect |
| `screen_and_press_agree_about_a_transparent_pixel` | `cli` | fails — **`screen [255,255,255] vs press [0,0,0]`**, defect (D) in one line |
| `correcting_an_assets_dpi_re_lays_the_pages_it_affects` | `session.rs` | fails at `assert_ne!` — the pages did not move |
| `relinking_to_a_differently_shaped_file_re_lays_the_pages` | `session.rs` | fails at `assert_ne!` |
| `editing_one_of_many_assets_reuses_the_pages_before_it` | `session.rs` | fails at `assert_ne!` |
| `flattened_pixels_are_still_ink_clamped` | `images.rs` | **passes** — spec 0006 must not regress |
| `an_image_without_alpha_is_untouched` | `images.rs` | **passes** — the no-change claim, on both paths |
| `an_unreadable_asset_falls_back_to_the_declaration` | `lib.rs` | **passes** — the probe's third answer |
| `an_untouched_asset_costs_nothing` | `session.rs` | **passes** — 0 blocks re-measured, every page reused |

The four passing-on-both-sides tests are what stop the other fourteen passing against a build that
flattens everything to white, warns about everything, or invalidates everything.

One test is **not** in that table and could not be: `probe_alpha_reports_what_the_file_carries`
exercises API that did not exist before this increment, so there was no defect for it to fail
against. It was disabled for the baseline run and is stated here rather than counted above. Its
value is different in kind — it pins the *dispatch*, so that every shape reaching an alpha-bearing
arm (`Rgba`, `GrayscaleAlpha`, keyed palette, keyed grey) answers `true`, every shape that cannot
answers `false`, and a file that is neither PNG nor JPEG answers `None` rather than `false`.

`quill-color` additionally pins the primitive itself: opaque is a byte-identical no-op, transparent
is paper whatever is stored underneath, coverage is monotone in alpha across all 256 values, and a
ragged buffer panics rather than mis-striding.

The cross-crate parity test lives in `crates/cli/tests/image_alpha_parity.rs` for
`hyphenation_parity.rs`'s reason: `cli` is the only crate that depends on both paths, so it is the
only place the two can be compared without inventing a dependency edge. It goes end to end on both
sides — a real export read back out of the PDF, and the real `lay_out_for_screen` → `paint_page` →
`rasterize` chain — and it samples the **opaque** half as well, which is what stops it passing
against a screen path that simply drew nothing.

## Risks

- **A logo drawn to sit on a dark ground is now visibly wrong instead of invisibly wrong.** White
  fringing where the author expected the page colour to show through is the honest consequence of a
  press file that cannot carry transparency. It is *visible*, on screen and in the file, and the
  preflight warning now names the backdrop. The real fix is a matte colour the author chooses, or
  PDF/X-4; both are named below.
- **`Proxy::rgba` is always opaque**, so any future consumer wanting real alpha must go back to the
  source. Recorded in the type's doc comment rather than left to be discovered.
- **The probe opens every linked asset at preflight.** One `File::open` and a header read per asset
  — no image data, no decode. For the 500-page art-heavy book that is hundreds of `stat`-scale
  operations against a preflight that already reads the ICC profile from disk. If it ever shows up,
  the answer is the proxy cache's `mtime + size` signature (spec 0024), not a return to trusting the
  declaration.
- **`content_fingerprint` now hashes fields layout does not read.** A `has_alpha` correction
  re-measures one image block to the identical height. Deliberate, argued above, and one block.
- **The naive conversion is still naive.** Compositing onto white is exact in 8-bit sRGB-ish
  samples; it is not gamma-correct compositing in a linear light space. Doing it in linear light
  would be more physically right and would differ from what every other DTP application does at
  8-bit, and — decisively — the screen path has no linear stage to share. One rule both paths run is
  worth more here than a better rule only one of them could.

## Non-goals

- **A matte colour other than paper.** `Asset` gains no field. Paper is the one backdrop a press file
  can justify without asking, and a per-asset matte is authored intent that belongs with the frame
  and fit work of spec 0085, where there is somewhere for it to live.
- **`/SMask` and live transparency.** Forbidden by PDF/X-1a:2001 and PDF/X-3:2002. It is spec 0089's
  subject, together with the conformance change it implies.
- **Auto-measuring `px_w`/`px_h`/`dpi`.** Spec 0009's non-goal, and argued above as *not* the same
  fix: the field the press check gates on is not in the header.
- **A preflight check that reports a declared pixel size the file contradicts.** A real follow-up and
  a genuinely useful one — it is a new `CheckId` with its own severity question (is a wrong declared
  size an error, when the placed geometry is already derived from the declaration?), not a
  parenthetical on this increment.
- **Alpha on any format other than PNG.** JPEG carries none. TIFF and PSD do, and they arrive with
  spec 0084 — which will find `flatten_over_paper` already built and already shared.
- **Threading the ink-limit preset through the clamp.** Spec 0049's named follow-up, unchanged by
  the composite now sitting upstream of it.
