# 0050 — Preflight over placed geometry

**Milestone:** M3 · **Status:** implemented

## Why

Two press defects quill could not see, both of which need a laid-out page rather than a document.

**Effective resolution.** `ImageResolution` checked `Asset.dpi` — the resolution the *author declared
about the source file*. A 300 dpi image scaled up to twice its natural size prints at 150 dpi and
passed. The real quantity is pixels divided by placed inches, and it is only knowable after layout.

**The live area.** Nothing checked that content stays clear of the trim by the printer's safety
margin. Text that is inside the page but 2 mm from the guillotine is exactly the defect spec 0036's
folio had — caught by eye, on a 2× render, while every numeric test passed — and a real book has
dozens of chances to reproduce it.

Both extend `preflight_pages`, which spec 0037 established as the pass that sees geometry the engine
synthesized, and both read their thresholds from spec 0049's preset.

## What

`preflight_pages` gains the page setup and the asset list. It needs the first to know where the trim
is and the second to know how many pixels an image has; neither is derivable from a placed page.

### Effective dpi is pixels over *placed* inches

```rust
fn under_dpi(asset: &Asset, frame: &Rect, preset: &PodPreset) -> Option<(f32, f32)>
```

Reads the laid-out rect and nothing else. Deriving the placed size from `Asset.dpi` would make the
check circular — it would be asking the field under suspicion whether it is telling the truth.

The finding names **both** numbers, the effective dpi and the required one. A preflight message that
does not say by how much you missed is a message the user cannot act on. Line art is held to the
preset's higher threshold.

### The live area, and the direction that matters

A block extending **outward** past the trim is a deliberate full bleed and is exempt. A block
falling **inward** of the live edge is at risk from the guillotine and is not.

That distinction is the whole check. False positives destroy a preflight faster than false
negatives, because a user who learns to skim the report skims the real finding too — so a full-bleed
background, which every art-heavy book has, must never be flagged.

Master statics are checked alongside flowed content: the folio that prompted this was furniture, not
content.

### Findings are per page, not per block

A 500-page document with one systematically misplaced running head reports it once per page. Once
per placed block would produce a report nobody reads, which is the same outcome as no check.

### `generic` states no safety margin, and that is deliberate

`PodPreset::generic().safety_pt` is `0.0`, which makes the safe-area check inert by default. This is
the same posture spec 0049 took with the vendor presets' trim catalogues, for the same reason: no
figure has been confirmed against a printer, and quill will not invent a requirement.

It is also not a nominal choice. `PageSetup::default()` has zero margins on purpose — spec 0036 gives
templates real ones instead, so the CI golden path never moves — which means the shipped
`Document::sample()` has content at the trim edge. A non-zero `safety_pt` in `generic` therefore
fails the sample, **correctly**, and that is precisely why the number has to come from a printer
rather than from quill. The mechanism ships and is tested against a preset that states a margin.

### A check missing from `CheckId::ALL`

`applied()` is derived from `ALL` minus the skip list, so a variant missing from `ALL` makes the
report understate what preflight covers. `TrimSize` had been missing since spec 0049. Fixed here,
with `SafeArea` added beside it, and a test asserting both are listed — the same class of defect as
a CI job that is not a required context.

## Acceptance criteria

- Regression: `Document::sample()`'s export byte-hash unchanged; it passes both new checks under
  `generic`, which is asserted rather than assumed.
- **Spec 0036's fore-edge folio is reproduced and now fails preflight.** This is the increment's
  proof of worth.
- Content well inside the live area passes; a full-bleed image is exempt; a preset with
  `safety_pt: 0.0` makes the check inert. All three asserted.
- A 300 dpi image placed at 2× reports an Error naming both the effective dpi (150) and the required
  one (300). The same image at 1× passes, and so does exactly the threshold.
- Line art is held to the preset's line-art threshold.
- Twenty intruding blocks on one page report one finding.
- Every `CheckId` variant appears in `CheckId::ALL`.

## Test strategy

The reproduce-the-0036-folio test is written first; the rest are boundary cases around it. The
bleed-exemption test is the one that would not be written by extension of the others, and it is the
one guarding the property that makes the check worth having.

## Risks

**False positives.** Guarded by the bleed exemption and by the per-page deduplication, and both have
their own tests.

**Circularity in the dpi check.** An image's placed size is derived from `px_w`/`px_h` and `dpi`
during layout, so a check that recovered the placed size from `dpi` would always agree with itself.
It reads the laid-out rect.

**A signature change.** `preflight_pages` is public and gained two parameters. Spec 0049 had already
widened it once; the alternative — re-laying the document out inside preflight — would give the
checks a *different* set of pages from the one the writer draws, which is the spec 0013 defect
exactly.
