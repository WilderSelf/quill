# 0063 — Inline runs: the paragraph stops being a `String`

**Milestone:** M5 · **Status:** implemented

## Why

`Block::Body` and `Block::Heading` each carried one `String` and one `Color`. There was no styled
run, span or inline formatting of any kind anywhere in the workspace, so quill could not set a single
word differently from the words beside it. Everything downstream inherited the assumption:
`measure_block` resolved one `ParagraphStyle` and passed one `font_size_pt` to the breaker;
`PlacedBlock::Text` carried one `color`; the PDF writer called `set_font` and `set_fill_*` once per
block, before the line loop; the screen painter converted colour once per block.

Nearly every absent typographic feature is downstream of this one. Character styles, drop caps, small
caps, OpenType feature control, tracking and baseline shift are all properties of a *run*, and the
model had none. So is the reason `quill import` never learned emphasis — there was nothing to import
it into.

## What

### The model

```rust
pub struct Run {
    pub text: String,
    #[serde(default, skip_serializing_if = "InlineStyle::is_empty")]
    pub style: InlineStyle,
}

pub struct InlineStyle {
    pub size_pt: Option<Pt>,
    pub color: Option<Color>,
    pub tracking_pt: Option<Pt>,
    pub baseline_shift_pt: Option<Pt>,
}
```

`Block::Body` and `Block::Heading` carry `runs: Vec<Run>` in place of `text: String`. The block keeps
its `color`: it is the paragraph's ink, and a run's `color` overrides it. A run whose `InlineStyle` is
empty is exactly the paragraph style resolved for the block, which is what makes the byte-identity
result below reachable at all.

Every field is an `Option` *override* rather than a value, so *absent* and *equal to the paragraph's*
stay distinguishable: editing a paragraph style must move a run that did not opt out, and must not
move one that did.

### What takes effect here, and what is declared

`color` takes effect end to end: measurement, the PDF writer, the screen painter, preflight and the
incremental cache all read it. `size_pt`, `tracking_pt` and `baseline_shift_pt` are declared here and
**take effect in spec 0064**, together with weight and italic.

That split is the increment's central scoping decision, and it is not arbitrary. The three
metric-bearing overrides all change what a *box* measures, so honouring them means per-box measurement
and per-glue widths inside the Knuth-Plass DP — every prefix sum, the shrink allowance, spec 0060's
natural-width rule and spec 0051's pruning monotonicity argument each take a size parameter they do
not currently have. Weight and italic are not a model problem at all: `quill-fonts` has exactly one
face. `Font::bundled()` parses one embedded `SourceSerif4-Regular.ttf`; nothing indexes faces by
weight or style; `shape`, `measure_run` and `ascent_pt` take a size and no face selector; and
`export-pdf` subsets one program and emits one PDF resource name, `F0`. Bold is a font *family* the
workspace does not have, plus multi-face subsetting and a resource dictionary with more than one
entry.

Both are one increment's work against a run model that already exists. Neither is provable in the
same breath as the run model itself, because both *move glyphs* — and the claim this increment has to
make is that nothing moved. So this one carries the structure, the span pipeline and the single
override that changes ink without changing metrics; 0064 carries the ones that change metrics.

### `Line` grows spans

`text-layout` stays colour-agnostic — it knows what changes a measurement, and a colour does not.
`Line` gains:

```rust
pub struct Span { pub run: usize, pub len_bytes: usize }
```

The spans partition the line's `text` in order, so a run boundary may fall mid-line, which is the case
the whole increment exists for. A paragraph of one run yields one span covering the line, and every
consumer then takes the path it took before runs existed.

They are tracked rather than recovered. Each box in the item stream carries the byte offset it starts
at in the concatenated paragraph, and the spans are emitted during the same walk that reconstructs a
line from its boxes — so a span map is exact by construction rather than reverse-engineered from the
output. A glue's space and a hyphenated break's `-` belong to the run of the box before them: neither
exists in the source, and splitting a span to hold a character nobody authored would be a distinction
with no consumer.

The runs are **one paragraph** to the breaker. A change of treatment must not be able to move a break,
or the run model would be a layout change rather than a generalization — and a word straddling a
boundary is one word, hyphenated as one, because the alternative would hyphenate `bold` and `face`
separately and produce breaks no reader could explain.

### Downstream

- `Measured::Text` and `PlacedBlock::Text` gain `run_colors`: each run's ink already folded with the
  paragraph's, resolved once so the writer and the painter cannot disagree about a run's colour — the
  same rule `Line::indent_pt` follows.
- The PDF writer emits `set_fill_*` per span **only when a line's spans really do disagree**, and
  takes the once-per-block path otherwise. The justification adjustment goes after every inter-word
  space exactly as before, emitted in whichever span's array the space falls, so a boundary inside a
  word cannot lose or double one.
- The screen painter emits one paint op per span, positioning each at the natural width of everything
  before it plus the justification already spent on the spaces behind it — the same accumulation the
  writer's `TJ` array performs, from the same metrics.
- `collect_doc_chars` walks every run's text, or a recoloured word would subset to `.notdef` boxes.
- Preflight reads every run's colour. One that read the block's and not its runs' would pass a
  document with an over-inked word in it.
- A fragment (spec 0044) keeps the whole run table in both halves: a span's `run` indexes the
  paragraph's runs, and re-basing per fragment would make a continuation's spans mean something
  different from its head's.

### The cache

`content_fingerprint` walks the runs, hashing each one's text with a boundary byte between them, so
`["ab"]` and `["a","b"]` cannot collide — they set the same characters but are not the same document,
since one of them can be recoloured mid-word. Run colours join the existing colour tail, which is what
makes a recoloured word invalidate the page it is on. (That tail forces a re-measure as well; that is
pre-existing behaviour for a block's colour, deliberately accepted to keep one key rather than two,
and this increment does not change it.)

### The format

`FORMAT_VERSION` 4, with `migrate_3_to_4` turning `"text": "…"` into `"runs": [{"text": "…"}]` for
every `heading` and `body` block. Unlike the two migrations before it this removes the old key rather
than defaulting a new one, because the run model is what `text` should have been and carrying both
would leave two sources of truth for the same characters. A `toc`'s title and a panel's fields are not
paragraphs and keep their strings.

## Results

- **`Document::sample()`'s export moved only by its identifiers.** Exported against the committed
  parity ICC before and after: **8786 bytes both sides**, 124 differing bytes, every one of them
  inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID`. Not one byte of a content, font or
  metadata stream moved. The `/ID` derives from `doc.to_json()`, which now carries `runs` and
  `format_version` 4, so an identifier-only diff is the expected result rather than a hopeful one.
- **No component geometry moved at all.** Spec 0054's parity corpus digests the `Debug` rendering of
  the placed pages, which necessarily moves when the structs grow a field. Stripping the two new
  fields textually reproduces the pre-0063 rendering exactly, and **every one of the ten constants
  still matches** — so the corpus now asserts twice: the geometry against the unmoved constants, and
  the structure against new ones, keeping the digest's original virtue that a field nobody thought to
  list cannot slip past it.
- text-layout's 34 pre-existing tests pass unchanged, which is the same claim from the other end:
  breaking is not a function of run structure.

## Acceptance criteria

- A single-run paragraph lays out identically to the same paragraph as a string — asserted directly,
  and again as the whole-corpus component-geometry result above.
- Splitting a paragraph into runs at arbitrary points, including mid-word, moves no break and no
  justification, over four measures.
- A word straddling a run boundary never breaks at the boundary, and the boundary shows as a span
  change inside the word.
- Every line's spans partition its text exactly, and reading them back reproduces each run's own text.
- A v3 document migrates to one plain run per paragraph; a `toc` title is untouched; a newer file is
  refused by name.
- A run's colour override reaches the content stream, and an over-inked run is caught by preflight
  naming the run.
- Recolouring a run invalidates the page it is on; changing a run's text re-measures; two runs are not
  the same content as one.

## Non-goals

- **Per-run size, tracking and baseline shift, and weight and italic** — spec 0064, with the font
  family they need. Declared in `InlineStyle` here so the format does not move twice.
- Character styles: a run carries overrides here and names a style in spec 0065.
- Rich text in a `Panel`'s fields, a `Table`'s cells or a declared component's sections: they are
  `String`s and stay so. This increment prepares that and does not do it.
- Per-run language, OpenType features, small caps, drop caps.
- `quill import` gaining emphasis: the importer's target is weight and italic, which is 0064.
