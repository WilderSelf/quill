# Quill file format (`.tpub`) — draft spec

**Status:** draft. The format is intentionally **open, documented, and versioned** — a
differentiator versus proprietary `.afpub` / `.indd`.

## Container

A `.tpub` file is a **Zip container** (like `.idml`/ODF). Layout:

```
document.json      # manifest + document model (see below)
assets/            # linked originals (images, etc.), referenced by relative path
fonts/             # fonts used by the document (for portability + embedding at export)
thumbnails/        # optional cached preview images
```

Rationale: linked assets (not inlined) keep the manifest small and diffable and keep large
art out of the model — the same principle that lets the editor stay fast on 500-page,
image-heavy books. The manifest is JSON (text) so projects are **git-friendly**.

## Manifest (`document.json`)

Top-level shape (the authoritative schema is the `serde` types in `quill-core-model`; this example
is **parsed by a test**, so it cannot drift from what the reader accepts):

```json
{
  "format_version": 10,
  "metadata": { "title": "The Ruined Keep", "authors": ["Anon"] },
  "page_setup": { "trim": { "w_pt": 468.0, "h_pt": 720.0 }, "bleed_pt": 9.0, "facing_pages": true,
                  "margins": { "top_pt": 36.0, "bottom_pt": 36.0, "inside_pt": 54.0, "outside_pt": 36.0 } },
  "master_pages": [
    { "name": "body", "columns": 2, "gutter_pt": 12.0,
      "margins": { "top_pt": 36.0, "bottom_pt": 36.0, "inside_pt": 54.0, "outside_pt": 36.0 },
      "statics": [
        { "kind": "text", "rect": { "x_pt": 54.0, "y_pt": 18.0, "w_pt": 378.0, "h_pt": 12.0 },
          "text": "{section}", "color": { "space": "gray", "v": 0.0 },
          "style": "folio", "align": "center" },
        { "kind": "text", "rect": { "x_pt": 54.0, "y_pt": 690.0, "w_pt": 378.0, "h_pt": 12.0 },
          "text": "{page}", "color": { "space": "gray", "v": 0.0 },
          "style": "folio", "align": "outside", "mirror": true }
      ] },
    { "name": "chapter-opener", "columns": 1, "gutter_pt": 0.0, "statics": [] }
  ],
  "default_master": "body",
  "pages": [ {}, {} ],
  "sections": [ { "name": "The Ruined Keep", "start": 1, "master": "chapter-opener" } ],
  "breaks": [ { "before": 1, "kind": "recto" } ],
  "styles": { "paragraph": {
    "body": { "font_size_pt": 10.0, "leading_pt": 12.0, "align": "justified" },
    "folio": { "font_size_pt": 9.0, "leading_pt": 12.0, "align": "left" } } },
  "content": [
    { "kind": "heading", "id": 1, "level": 1,
      "runs": [ { "text": "The Ruined Keep" } ],
      "color": { "space": "gray", "v": 0.0 } },
    { "kind": "body", "id": 2,
      "runs": [ { "text": "A dank corridor stretches into " },
                { "text": "darkness", "style": { "italic": true } },
                { "text": "; see page " },
                { "text": "[?]", "source": { "kind": "reference", "target": 3 } },
                { "text": "." } ],
      "color": { "space": "cmyk", "c": 0.0, "m": 0.0, "y": 0.0, "k": 1.0 } },
    { "kind": "image", "id": 3, "asset": "map1" },
    { "kind": "body", "id": 4,
      "runs": [ { "text": "The keep is older than the town" },
                { "text": "[?]", "source": { "kind": "footnote", "note": 5 } },
                { "text": "." } ],
      "color": { "space": "gray", "v": 0.0 } }
  ],
  "footnotes": [
    { "id": 5, "runs": [ { "text": "So the parish rolls claim." } ],
      "color": { "space": "gray", "v": 0.0 } }
  ],
  "revision": 0,
  "next_block_id": 6,
  "assets": [ { "id": "map1", "path": "assets/map1.png", "px_w": 1500, "px_h": 1200, "dpi": 300.0 } ]
}
```

## Margins and master pages

Margins are `inside`/`outside`, not left/right: a bound book's margins are relative to the spine, so
the inside margin falls on the left of a recto and the right of a verso. A `MasterPage` names the
margins, column count, gutter and repeating furniture shared by the pages it governs; a static's text
may carry **tokens** resolved when the page is laid out (below). `default_master` names the master applied to
the document; an unknown name degrades to the document's own page setup rather than refusing to open
the file.

`pages` (spec 0035) overrides that default per page, positionally — `pages[i]` governs page `i`.
A page's master is its own override, else `default_master`, else none, and a name matching no master
falls through to the next step rather than failing, on the same principle: a renamed master costs
the page its furniture, never the author their page. The list need not match the document's length;
pages beyond it fall back, and entries beyond the document are ignored. The list is omitted from the
manifest entirely when empty, so a document that never assigns a master reads exactly as it did
before spec 0035 — which is why this addition needed no `format_version` bump.

Because assignment is positional, content that pushes the book by a page slides every subsequent
assignment. That was the accepted trade for M2, and spec 0072 removes it — see below.

## Sections (spec 0072, v5)

A **section** is a named division of the document anchored to the block that opens it:

```json
"sections": [ { "name": "The Ruined Keep", "start": 1, "master": "chapter-opener" } ]
```

- `name` — what the section is called. Authored rather than taken from the anchor's text, because a
  running head ("The Ruined Keep") and a chapter opener ("Chapter One: The Ruined Keep") are
  routinely different strings.
- `start` — the `id` of the block the section opens with. Any block, not only a heading.
- `master` — optional; the master applied to the section's **opening page**.

A section does not replace the `pages` list: it **generates** it. Each layout pass reads the page
each anchor was placed on, and synthesises the same `Vec<PageOverride>` described above from those
page numbers, overlaid on whatever the document authored positionally. Resolution is unchanged —
`pages[i]`, else `default_master`, else the document's page setup — so a section is an *authoring
surface* over the representation the format already had.

That is what makes the assignment survive repagination: a block id is stable across every edit that
does not delete the block, so inserting a chapter in front of another moves the second chapter's
opener with it, where a positional entry would stay on the page number it names. Where a section and
a positional entry claim the same page, the section wins — it is the one that tracked the content.

Two consequences worth stating:

- **Master assignment is part of the layout fixpoint.** A section's opener master changes that
  page's margins and column count, which changes where later content falls, which can move the
  anchor. Layout therefore iterates — sharing the contents list's loop and its bound (8 passes) —
  and reports whether it settled rather than presenting the last iterate as an answer.
- **A section is a marker, not a container.** It does not own its blocks; there is no tree, and a
  section runs until the next section's anchor. An anchor naming a block the document does not
  contain is not an error: the section is simply not placed, on the same principle a dangling master
  name falls through rather than failing.

A template file carries no sections and cannot: it has no content, therefore no block ids to anchor
to. This is why `TEMPLATE_VERSION` did not move when `format_version` became 5.

Not in this increment, and named rather than forgotten: **"start this section on the next recto"**.
That is a forced page break, which the model has no mechanism for at all, and it is a forward-only
rule rather than a fixpoint.

## Layout-time tokens in a master static (specs 0029, 0073, 0074)

A text static's `text` is a template, not a literal. Three tokens are replaced when the page is laid
out, and everything else is printed as written:

| Token | Resolves to |
|---|---|
| `{page}` | The page's **folio** — the number printed on it (see Folios below). One-based arabic wherever no section says otherwise, which is what every document got before spec 0073. |
| `{section}` | The `name` of the section the page belongs to — the last section whose anchor was placed at or before this page. Empty on a page ahead of every section; a half-title does not borrow chapter one's name. |
| `{heading:N}` | The text of the last heading of level ≤ `N` at or before this page. `{heading:1}` is "the current chapter"; `{heading:2}` follows sub-sections too. Empty before the first qualifying heading. |

So a running head is authored once on the master rather than typed onto a per-page master for every
chapter, which is what `pages`-based assignment forced before sections existed.

Three consequences worth stating, because each is a decision rather than a detail:

- **`{heading:N}` prints flattened text.** The heading index carries a heading's characters, not its
  runs, so a chapter titled "The *Ruined* Keep" appears in the running head with the italic lost.
  Named as a residual by spec 0074 rather than half-fixed.
- **A `{…}` group that spells no token is printed literally.** That is deliberate: it is the same
  posture a dangling master name and a dangling style name already have, and it is what makes an
  older build meeting a newer token *loud* — it prints `{section}` on the page, visibly wrong, rather
  than something plausible and wrong. It is also why adding a token needs no `format_version` bump:
  the rule below turns on *silence*.
- **A template file may carry these tokens**, since it carries master pages. A template has no
  sections and no content of its own, so what they resolve to is decided by the document the template
  is used for.

## Folios: what a page prints as its number (spec 0073, v6)

A `Section` may state a `folio`, and it governs the section's **whole run** — every page from its
opener until the next section that states one — because that is what page numbering is:

```json
"sections": [ { "name": "Front matter", "start": 1, "folio": { "format": "lower_roman", "restart_at": 1 } } ]
```

- `format` — `decimal` (the default), `lower_roman`, `upper_roman`, `lower_alpha`, `upper_alpha`.
  The same `NumberFormat` a list marker uses; there is one roman numeral converter in the workspace,
  not two.
- `restart_at` — omit to carry the count on from the page before (what a section that changes only
  the *format* wants); `1` is the restart a part opener asks for; `n` is the offset a chapter
  extracted from a larger book needs.

A contents entry prints the folio too, because a list that says `4` for a page printed `iv` sends the
reader to the wrong page. `/Link` and PDF outline **destinations** keep the physical page index,
because a destination is a reference to the *n*th page of the file and a viewer resolves it
positionally. Both are page numbers; they are not the same page number.

## Cross-references: "see page 42" (spec 0076, v7)

A **run** may say where its characters come from. `source` is absent on every authored run — which is
every run written before v7 — and a run that carries one draws something generated at layout time
instead of its own `text`:

```json
{ "text": "[?]", "source": { "kind": "reference", "target": 3 } }
```

- `target` — the `id` of the block being referred to. **Any block**, not only a heading: "see the
  table on page 42" is the same sentence as "see the chapter on page 42", and the anchor mechanism
  is general for free.
- What it prints is the **folio** of the page that block landed on — the number a reader is told, on
  the same split spec 0073 drew for a contents entry. A `/Link` destination still carries the page
  *index*.
- `text` is **not** a cached page number and is never read while laying out. A stored number is stale
  the moment anything is edited, which is spec 0041's rule for a contents list applied to a run. It
  holds the unresolved marker below, so a build that does not understand `source` prints the same
  thing this one prints for a reference it cannot resolve.

**A target that is not in the document prints `[?]`.** That is a decision, and it is deliberately
*not* the posture a dangling section anchor or a dangling master name gets. Those lose furniture,
which is recoverable and visible by comparison across pages; a cross-reference is **content, in the
text flow**, so rendering nothing would leave "see page ." in a sentence that reads as finished.
Refusing to open the document was the other option, and it loses a whole book to one stale id —
which spec 0072 already rejected. A marker survives to the proof and reads as unfinished, which is
what `CLAUDE.md`'s "prefer a visible failure" means for running text.

**A cross-reference is genuinely in the flow, so layout iterates over it.** "See page 142" is three
digits where "see page 42" is two, so its width moves a line break, which moves a page, which moves
the number. It joins the same bounded fixpoint the contents list and the sections run in, and a
document that will not settle reports `converged: false` and ships its last iterate rather than
presenting a guess as settled. Unlike a folio, this one really can oscillate: roman numerals are not
monotone in width (`viii` is wider than `ix`), so a reference into roman-numbered pages can push its
own target back and forth across a page boundary for ever.

## Footnotes (spec 0077, v8)

A footnote is an **anchor** in the text flow and note text set in a band at the foot of the frame the
anchor landed in. The note is not in `content` — putting it there would set it inline, which is the
thing a footnote exists not to do — so a document gains a second list:

```json
"footnotes": [ { "id": 5, "runs": [ { "text": "So the parish rolls claim." } ],
                 "color": { "space": "gray", "v": 0.0 } } ]
```

and a run may name one, through the same `source` field a cross-reference uses:

```json
{ "text": "[?]", "source": { "kind": "footnote", "note": 5 } }
```

- **`footnotes` is a store, not a second content sequence.** Order in the list is *not* the
  numbering, so re-ordering it changes nothing a reader sees. A note nothing anchors is not an error
  and is simply not placed, on the same principle a dangling section anchor is not placed.
- **Ids are one space.** A footnote's `id` comes from the same `next_block_id` counter the blocks
  use, and a footnote sharing an id with a block is refused at load exactly as two blocks sharing one
  are.
- **Numbering is document-sequential and decimal**, derived from the order the *anchors* appear in
  `content`: 1, 2, 3… assigned the first time an anchor names a note. It is knowable before anything
  is laid out, which is what keeps a footnote out of the layout fixpoint entirely — a page-restarting
  number would not be, since the page is not known until the flow has run and the number's width
  changes where the anchor's paragraph breaks. Per-page restart is a named non-goal in
  `specs/0077-footnotes.md`, with what it would cost.
- **The anchor's `text` is not a cached number**, for the reason a cross-reference's is not. It holds
  the unresolved marker, so an anchor naming a note the document does not hold prints `[?]` — and so
  does a build that does not understand `source`.
- **A note is set in the `footnote` paragraph style** unless it names another. The style is in the
  default sheet, so a document gets a conventional treatment without authoring one; a sheet that does
  not define it falls through to `body`, which is what a renamed style already does.
- **A long note splits across pages.** The remainder continues at the foot of the next frame, through
  the same mechanism a paragraph splits by.
- **The band is per *frame*, not per page.** On a two-column page a note sits at the foot of the
  column its anchor is in. A footnote area spanning the columns of a page is a named non-goal: it is
  not in any frame, and the flow reserves space in frames.

## The index (spec 0078, v9)

A back-of-book index is a **generated block** plus **marks on runs**. The block stores no entries,
for the reason a `toc` stores none: they are derived from the pages the marked runs actually landed
on, and a stored entry is a page number that was right one edit ago.

```json
{ "kind": "index", "id": 9, "title": "Index",
  "ignore_leading": ["A", "An", "The"],
  "color": { "space": "gray", "v": 0.0 } }
```

and a run says what it puts in the index, beside `style`, `character` and `source` rather than inside
any of them:

```json
{ "text": "the routed militia broke", "index": { "term": "morale", "sort_as": null } }
```

- **The mark is not a `source`.** `source`'s variants are mutually exclusive — a run draws its own
  characters, or a reference's folio, or a footnote's number — and a mark is not that kind of thing:
  one run can be both a cross-reference and an index term. It is an annotation on a run, which is the
  slot `character` occupies.
- **`term` is authored, not taken from the run's text.** That is what lets a paragraph about routing
  be indexed under "morale", and a normally-set sentence be filed as "Keep, the Ruined".
- **`sort_as`** is what the term files under when that is not the term itself: `de Gaulle, Charles`
  under `Gaulle`, `1984` under `Nineteen Eighty-Four`. No collation rule can derive those, because
  they are facts about the language and the author's intent rather than about the characters.
- **A term's page is the page its own *run* landed on**, not the first page of its block. A
  paragraph cut across a page boundary reports a term marked in its last sentence on the page that
  sentence is on.
- **A term marked twice on one page appears once**, and two marks with the same `term` are one
  entry however they are spelled.
- **The entries print folios**, and consecutive folios coalesce: `iv, 42–45, 47`. Coalescing is over
  the *numbering* (format plus number), not the printed strings and not the page indices, so a
  section boundary that changes the format or restarts the count breaks the run — `ix` and `1` are
  never printed as a range.
- **An index longer than a frame splits between entries**, through the same mechanism a contents
  list and a table split by. Its own title is stated once and never repeated on a continuation.
- **It joins the layout fixpoint**, as the contents list does, and reports `converged: false` rather
  than presenting a last iterate as settled.

### Collation

The order terms are filed in is a **named, tested rule**, not byte order. `BTreeMap<String, _>` would
file `Zebra` before `apple`, `Zürich` after `Zyklon`, and `The Ruined Keep` under T.

A term's key is four levels, compared in order: base letters (case folded, accents folded,
everything that is neither a letter nor a digit ignored, digits sorting ahead of letters); then the
accents that were folded away; then case, lower before upper; then the term itself, so the order is
total and collating a set is deterministic.

Two limits are stated rather than left to be discovered:

- **Filing is letter-by-letter**, so `New York` files after `Newark`. Word-by-word filing is the
  other convention publishing sanctions and is not implemented.
- **The rule is not language-tailored.** It is right wherever a script's alphabetical order agrees
  with its Unicode order after folding — which covers Latin, Greek and Cyrillic — and wrong wherever
  a language *tailors* that order, as Swedish does by filing `å ä ö` after `z`. `ignore_leading` and
  `sort_as` cover the two exceptions a book can state for itself; a language-wide tailoring needs a
  Unicode collator, and `specs/0078-index.md` prices that.

`ignore_leading` is **empty by default**: quill never guesses a language it was not told. A word
there is an article only when the term continues with a space or an apostrophe, so `Theatre` is not
`The` + `atre`, and the longest match wins so `["A", "An"]` cannot file `An Atlas` under `n`.

## Forced page breaks (spec 0080, v10)

A **break** says that the content at a block resumes on a new page rather than in the frame the flow
had reached. It is what a chapter opener is, and — with `recto` — what "start this section on the
next recto" asks for.

```json
"breaks": [ { "before": 1, "kind": "recto" } ]
```

- `before` is the [`BlockId`](#block-identity) the break sits in front of. An id the document does
  not contain is **not** an error: the break is not applied, the posture a dangling section anchor
  and a dangling master name already have.
- `kind` is `page` (the default, and what an entry with no `kind` means), `recto`, `verso` or
  `none`. `recto` and `verso` are one parity rule with the two answers, resolved through the same
  `is_recto` that decides which way a margin mirrors; with `facing_pages: false` every kind accepts
  every page, because a single-sided document has no spread. `none` is a break that asks for
  nothing, and exists so a *book file* can decline the break its chapters get by default.

**A break is anchored, not a field on the block, and not a block of its own.** A `page-break` block
would be a block with no content that every consumer has to skip, and deleting the heading it was
written for would leave it behind, breaking the page in front of whatever moved up. A field on the
block would be inside the value the measurement cache keys on — and a break changes *where* a block
goes, never *what it measures*, so it must not re-break a paragraph that has not moved. Anchoring
makes that unrepresentable rather than merely avoided. It is the same shape spec 0066 found for a
list: a property of a paragraph, not a kind of block.

**What a break does to the pages, exactly:**

- The flow advances **pages**, never frames — on a two-column page a break in the first column does
  not settle for the second, because the page still carries what came before.
- A break at the top of a page the flow has not written to is a **no-op**. Without that rule every
  break would insert a blank page, and a chapter opened on its own would begin on page 2.
- A parity break may insert **one blank page**, and a blank page is a page in every other respect:
  it takes its master, prints its folio and its running head, and **consumes a page number**. A page
  that did not would leave every folio after it one out.

## Master statics: alignment and page parity (spec 0047, v3)

A text static carries two optional fields, both defaulting to the pre-v3 behavior and both omitted
from the manifest when they do:

- `align` — `left` (default), `center`, `right`, `inside`, `outside`. Where the line sits **within
  its rect**. `inside`/`outside` resolve to left/right by page parity, the same rule the margins
  use: a recto has the spine on its left, and with `facing_pages` off every page is a recto.
  A static is one unbroken line, so there is no `justified`.
- `mirror` — when true, the rect itself is reflected about the page's vertical centre on a verso
  (`x' = trim.w - (x + w)`). The rect is authored **as it looks on a recto**, exactly as margins are
  authored inside/outside.

Both are needed to place a folio at the outside corner of a spread, because the band it sits in is
itself asymmetric whenever the inside and outside margins differ: `mirror` moves the band to the
right half of the page, `align` puts the number at its fore-edge end. Alignment is resolved at
layout, against the same font metrics the text is broken with, so a placed static's frame reports
the measured line rather than the band it was aligned in.

## Versioning

`format_version` is an integer. Readers reject formats newer than they understand and migrate
older ones forward. Migrations are documented per bump.

Implemented in `quill-core-model` (spec 0025): the gate runs on the untyped JSON *before*
deserialization, since an older manifest by definition does not fit the current `serde` types. A
manifest newer than this build is refused with `LoadError::UnsupportedVersion` rather than loaded
with its unknown fields dropped — a half-loaded document saved back over the original would destroy
whatever this build did not understand.

| `format_version` | Behavior |
|---|---|
| absent | treated as current |
| older | migrated forward, one step per version |
| current | loaded as-is |
| newer | refused |

### The migrations

Each step brings a manifest forward exactly one version and falls through to the next, so a
document written by any released build reaches the current types in one load. All but one are
structurally no-ops — every field they add is `serde(default)` and the default is what the older
version *meant* — and they are written out anyway, so the chain reads as a record of what each
version changed.

| From → to | Spec | What changed | Migration |
|---|---|---|---|
| 1 → 2 | 0030 | `page_setup.margins`, `master_pages`, `default_master` | defaults `margins` to zero on every edge and `master_pages` to empty: a v1 document had no margins and no masters |
| 2 → 3 | 0047 | `align` and `mirror` on a text master static | defaults `align` to `left` and `mirror` to `false` on every text static: a v2 static was drawn from its rect's left edge in the same rect on both halves of a spread |
| 3 → 4 | 0063 | a paragraph's single `text` becomes an ordered list of styled `runs` | **the one migration that is not a structural no-op**: `text` is removed from every `heading` and `body` block and replaced by `runs: [{"text": …}]`. A `toc` title and a panel's fields are not paragraphs and keep their strings. Still a serialization no-op *in effect* — one plain run carries no style object — so a migrated v3 document lays out and exports as it did |
| 4 → 5 | 0072 | `sections` | defaults `sections` to empty: a v4 document had no sections, and every master assignment it had was positional |
| 5 → 6 | 0073 | `folio` on a section | defaults `folio` to absent on every section: a v5 document numbered every page arabic from 1, which is what a section stating no folio still means |
| 6 → 7 | 0076 | `source` on a run — a cross-reference to a block | defaults `source` to `authored` on every run: a v6 run drew its own `text`, which is what an absent `source` still means. Nothing is written into the manifest, for the reason 5 → 6 writes nothing |
| 7 → 8 | 0077 | `footnotes`, and a `footnote` anchor on a run's `source` | defaults `footnotes` to empty: a v7 document had no notes, and no run could name one. Nothing is written into the manifest, for the reason 6 → 7 writes nothing |
| 8 → 9 | 0078 | `index` on a run, and the `index` block | defaults `index` to absent on every run: a v8 document marked nothing, which is what an absent `index` still means. Nothing is written into the manifest, for the reason 7 → 8 writes nothing |
| 9 → 10 | 0080 | `breaks` — forced page breaks anchored to blocks | defaults `breaks` to empty: a v9 document breaks nowhere, which is what an absent list still means. Nothing is written into the manifest, for the reason 8 → 9 writes nothing |

A bump is warranted whenever an **older** build would open the document and silently lay it out
wrongly, even when the new fields are optional. A pre-0030 build would drop a document's running
heads and column geometry; a pre-0047 build would put every verso's folio in the gutter; a pre-0072
build would drop `sections` and set every chapter opener in the body master; a pre-0073 build reads
`sections` perfectly well, ignores `folio`, and numbers the front matter arabic. Refusing to open beats
any of them, because a half-understood document can be saved back over the original — and the loss
is worse for authored intent than for derived state: block ids can be regenerated, a section list
cannot.

**Spec 0076 is the case where the two halves of the rule split, and it is worth stating because the
first half does not fire.** A pre-0076 build meeting a cross-reference prints the run's stored
`text` — the `[?]` marker — so the book is *visibly* unfinished wherever a reference was, which is
the loud condition spec 0074's entry below turns on. What decides it is the second half: `source` is
model, it is authored intent (which block the author pointed at), it cannot be regenerated from
anything left in the file, and an older build that opens and saves destroys every reference in the
document permanently. 0074's other half — "nothing is added to the model, so no version gate could
have helped" — is false here. Loudness mitigates the proof; it does not undo the deletion.
`format_version` is **7**.

**Spec 0077 is the same split, and the loss half is heavier still.** A pre-0077 build meeting a
footnote prints `[?]` where the anchor was and drops `footnotes` as an unknown key, so the notes
visibly vanish — loud, again, on the first half of the rule. What fires it is that `footnotes` is not
derived state and not even intent: it is **prose**, the author's own sentences, and one open-and-save
deletes every one of them with nothing left in the file to regenerate them from. 0076 bumped for
losing which block a reference pointed at; this loses the note. `format_version` is **8**.

**Spec 0078 fires on both halves at once, and the one that decides it is the quiet one.** A pre-0078
build meeting an `index` *block* refuses the document outright — `kind` is a tagged enum and `index`
is a tag it does not have — which is as loud as a failure gets. But the marks are not in the block;
they are on the *runs*, where a pre-0078 build drops them as unknown keys, lays the book out with no
visible difference anywhere, and deletes the entire index on the first save. Which terms an author
chose to index, and what each files under, is authored intent in precisely 0076's sense: nothing left
in the file can regenerate it. `format_version` is **9**.

Note what the rule does *not* turn on. Not "did a struct gain a field" — spec 0035's `pages`, spec
0054's `components` and spec 0056's `requires` were all additive without a bump, because an older
build that drops them lays the document out exactly as the author would have got before writing
them. And not "is the field optional" — every field in the table above is optional too. The question
is only whether the older build's silence produces a *different book*.

Spec 0074 is the worked example of the rule *not* firing, and it is worth keeping because the
temptation is real: `{section}` and `{heading:N}` are a genuinely new capability, and a build that
predates them prints the literal text `{section}` on every page. That is wrong output — and it is
**loud**, on screen and in the press file, which is the opposite of the condition above. Nothing is
added to the model, either: the tokens live inside a `text` string a user can type at any time, so no
version gate could stop them arriving. `format_version` stays **6**.

**Spec 0079 is the rule not firing for a different reason: there is no document to be wrong about.**
A book is a new artifact with its own `book_version` (below), and it adds no field to `Document`,
`StyleSheet`, `MasterPage`, `PageSetup` or `PageOverride`. A build that predates books, handed a
`.qbook`, does not recognise the extension and never opens it — neither half of the rule can fire,
because there is no silence to be wrong in and nothing of the author's to lose. The chapters
themselves are ordinary v9 `.tpub`s that an older build opens exactly as it always did.
`format_version` stays **9**.

**Spec 0080 is the silence half at its plainest, and the loss half rides with it.** A pre-0080 build
meeting a v10 document drops `breaks` as an unknown key and sets the whole book as one continuous run
of text: every chapter opener lands halfway down the page the previous chapter ended on, a
chapter-opener master is applied to a page still carrying the previous chapter's tail, and every
recto opener falls on whichever side the flow happened to reach. Nothing is missing, nothing errors,
and nothing on the page says so — which is the failure the first half of the rule exists for. The
second half fires with it, in 0076's sense exactly: a break is authored intent ("this chapter opens
right"), nothing left in the file can regenerate it, and one open-and-save through an older build
deletes the structure of the book. `format_version` is **10**.

> **Bumping `FORMAT_VERSION`? Check whether `TEMPLATE_VERSION` is owed one too.** A template file
> (below) embeds `page_setup`, `styles`, `master_pages` and `pages`. A document bump that changes
> the serialized shape of any of those four changes every template file as well, and the template
> format has its own version and its own migration chain. Spec 0047's 2 → 3 was exactly such a bump.
> Spec 0072's 4 → 5 was checked against this and is **not**: it adds a field to the document beside
> those four and changes none of them, and a template cannot carry a section anyway. Spec 0078's
> 8 → 9 was checked too and is **not**: a `Run` is in none of the four (a master static's text is a
> `String`, not runs), the `index` block is content and a template has none, and the two new
> `index-*` paragraph styles add entries to a map without changing `StyleSheet`'s shape — the same
> reasoning specs 0041, 0066 and 0077 applied to their own built-in styles. A template written
> before them simply resolves them through the defaults, exactly as it does `toc-*`.
> Spec 0080's 9 → 10 was checked and is **not** owed one either: `breaks` is a field on the document
> beside those four and changes none of them, and a template file has no content, therefore no block
> ids for a break to be anchored to — the same argument that kept a section out of a template.

## Template files (spec 0053)

A **template file** is everything a document has except its content: trim, margins, a type scale,
master pages and the per-page assignments. `quill new --from house-style.json -o book.tpub` starts a
document from one. It is a separate published format from `.tpub` — a plain JSON file, not a
container, because a template links no assets.

```json
{
  "template_version": 1,
  "name": "house-style",
  "title": "House style (6×9, two columns)",
  "description": "The house two-column 6×9, with a chapter opener and page numbers.",
  "page_setup": { "trim": { "w_pt": 432.0, "h_pt": 648.0 }, "bleed_pt": 9.0, "facing_pages": true,
                  "margins": { "top_pt": 54.0, "bottom_pt": 54.0, "inside_pt": 54.0, "outside_pt": 40.0 } },
  "styles": { "paragraph": {
    "body": { "font_size_pt": 9.5, "leading_pt": 12.5, "align": "justified" },
    "h1": { "font_size_pt": 22.0, "leading_pt": 26.0, "align": "left", "space_before_pt": 19.5 },
    "folio": { "font_size_pt": 8.5, "leading_pt": 12.5, "align": "left" } } },
  "master_pages": [
    { "name": "chapter-opener", "columns": 2, "gutter_pt": 14.0,
      "margins": { "top_pt": 216.0, "bottom_pt": 54.0, "inside_pt": 54.0, "outside_pt": 40.0 },
      "statics": [] },
    { "name": "body", "columns": 2, "gutter_pt": 14.0,
      "margins": { "top_pt": 54.0, "bottom_pt": 54.0, "inside_pt": 54.0, "outside_pt": 40.0 },
      "statics": [
        { "kind": "text", "rect": { "x_pt": 54.0, "y_pt": 606.0, "w_pt": 338.0, "h_pt": 12.0 },
          "text": "{page}", "color": { "space": "gray", "v": 0.0 },
          "style": "folio", "align": "outside", "mirror": true }
      ] }
  ],
  "default_master": "body",
  "pages": [ { "master": "chapter-opener" } ]
}
```

The example above is parsed by a test, so it cannot drift from what the loader accepts.

`name` is the slug the success line reports; `title` and `description` are what `quill new --list`
shows for the bundled three. `styles`, `master_pages`, `default_master` and `pages` all default, so
the shortest useful template is a `page_setup` and a name — but `styles` is built on the default
sheet whenever it is omitted, so a template can never be missing a style the resolver expects. A
style name nothing resolves falls back to `body` and then to the default treatment: losing a
paragraph's *styling* beats losing the paragraph, which is the same posture a dangling master name
gets.

### Versioning a template file

`template_version` is an integer, and a **separate one** from the document's `format_version`: a
template file is not a document, and coupling them would re-version every template ever written
whenever the document model changed in a way templates never see. The rules are otherwise identical
to the document's — the gate runs on the untyped JSON before deserialization, and a file newer than
this build is refused (`LoadError::UnsupportedTemplateVersion`) rather than half-loaded.

| `template_version` | Behavior |
|---|---|
| absent | treated as current |
| older | migrated forward, one step per version |
| current | loaded as-is |
| newer | refused |

`TEMPLATE_VERSION` is **1** and the migration chain is empty, because nothing is older than v1. It
is owed a bump on either of two triggers:

1. the template envelope changes — a field added to or removed from the template itself; or
2. a `format_version` bump changes the serialized shape of `page_setup`, `styles`, `master_pages` or
   `pages`, the four document structures a template file embeds.

Trigger 2 is the one that is easy to miss, which is why it is also flagged beside the document
migration table above.

### Composing a template with a POD preset

A POD preset (spec 0049) carries the printer's geometry; a template carries a design. Both can state
a trim, so the precedence is fixed rather than left to the invocation:

- **Trim: the template wins.** A template's furniture is authored against a specific trim — a folio's
  `y_pt` is derived from the page height and its rect from the trim width. Re-trimming from
  underneath moves the page without moving the geometry authored for it, and nothing at layout time
  catches it, because furniture does not participate in the flow. A preset whose trim differs is
  *reported*; an unusual trim is a conversation with the printer, not a corrupt file.
- **Bleed: the larger of the two wins.** Bleed is a floor, not a design choice, and it lives entirely
  outside the trim box — so honoring a stricter press requirement costs the design nothing, and
  lowering it would cost something.

With no template at all, a preset seeds both outright.

## Book files (spec 0079)

A **book file** is several `.tpub` chapters composed into one press file: one page-number sequence,
one contents list, one set of embedded subsets, one OutputIntent, one preflight. It is plain JSON
with the extension `.qbook` — not a container, for the reason a template file is not one: a book
links no assets of its own, and its chapters carry theirs.

```json
{
  "book_version": 2,
  "metadata": { "title": "The Ruined Keep", "authors": ["A. Cartographer"] },
  "requires": [ { "name": "house-style", "version": "1" } ],
  "chapters": [
    { "path": "front.tpub", "name": "Front matter",
      "folio": { "format": "lower_roman", "restart_at": 1 } },
    { "path": "ch1.tpub", "name": "The Ruined Keep",
      "folio": { "format": "decimal", "restart_at": 1 }, "break_before": "recto" },
    { "path": "ch2.tpub", "name": "The Sunken Vault", "break_before": "recto" }
  ]
}
```

- `path` is relative to the book file. Absolute paths and paths escaping the book's own directory are
  refused, not sanitized — the rule a `.tpub` entry and a `.qpack` path already get.
- `name` is the chapter's name **as a section**: what `{section}` prints in a running head.
- `folio` is the `Folio` above, unchanged, and this is what `restart_at`'s "the offset a chapter
  extracted from a larger book needs" meant.
- `master` names the master for the chapter's opening page; omitted, the chapter's own positional
  override for its own page 0 supplies it.
- `break_before` (spec 0080) is the page the chapter opens on — `page` (the default), `recto`,
  `verso` or `none`. It becomes a `breaks` entry on the composed document, anchored to the same block
  the chapter's `Section` is. The first chapter's costs nothing whatever it says, because page 0 has
  received no content and a break at the top of an untouched page is a no-op.
- `requires` is spec 0056's list. **Shared styles are `.qpack` and nothing else**, and a requirement
  that does not resolve refuses the book rather than falling back.

**A book resolves to one `Document` before anything lays it out.** Content is concatenated, block ids
are rebased so no two chapters collide, asset ids are namespaced (`0/map`) and asset paths repointed
under `chapters/<i>/`, styles/masters/definitions merge first-wins, and one `Section` is synthesised
per chapter. Everything downstream — the layout fixpoint, the incremental session, the font-subset
collector, geometry preflight, the PDF/X writer — is unchanged, which is the point: those are the
paths that have been tested since M0.

Consequences worth stating, because each is a decision:

- **Pagination is continuous with nothing stated.** The composed document is one page vector, so the
  run at page 0 already numbers every chapter in sequence. There is no page-number offset anywhere; a
  book that wants a break says so with a chapter `folio`.
- **A chapter starts on a new page** (spec 0080), because `compose` gives every chapter a `breaks`
  entry. A book states nothing to get it; `break_before: "recto"` gets the opener a printed book
  usually wants, inserting a blank page where one is needed, and `"none"` declines it for parts that
  genuinely run on.
- **Every chapter must share one `trim`, `bleed` and `facing_pages`,** or the book is refused: a press
  file whose pages are not one trim is not a press file. Differing margins or baseline grids are
  settled by the first chapter and *reported*.
- **A cross-reference cannot reach another chapter.** A `BlockId` is unique within a document, so
  chapter 1's block 12 and chapter 3's block 12 are both block 12; rebasing shifts a chapter's ids and
  its references by the same offset, so a reference always resolves to the block it meant, and no id
  an author could write reaches out of its own file. A qualified target is the path, and it is a
  `format_version` decision of its own.
- **Positional page overrides for a chapter's later pages are dropped and reported.** They name
  positions in a document that no longer starts at page 1.

### Versioning a book file

`book_version` is an integer and a **separate one** from `format_version`, `template_version` and
`pack_version`, on the reasoning that separated the template's: a book file is not a document. The
rules are identical to the other three — the gate runs on the untyped JSON before deserialization,
and a file newer than this build is refused (`LoadError::UnsupportedBookVersion`) rather than
half-loaded.

| `book_version` | Behavior |
|---|---|
| absent | treated as current |
| older | migrated forward, one step per version |
| current | loaded as-is |
| newer | refused |

`BOOK_VERSION` is **2**. Unlike a template file, a book file embeds no document structure at all — it
names chapters by path and states a `Folio` per chapter — so a `format_version` bump does not
automatically owe one here. What owes a bump is a change to the book envelope itself, and spec 0080's
`break_before` is the first: a v1 build handed a book that says `"break_before": "recto"` drops the
key as unknown and opens every chapter on whichever side the flow reached, silently, which is the
document chain's own rule applied to this envelope. The 1 → 2 migration is a structural no-op — the
absent field already means `page`, which is what a book has always meant and what spec 0079 could not
yet do.

## Reading a container

`Tpub::read_manifest` reads `document.json` alone; `Tpub::open_into` also extracts the payload to a
caller-named directory and returns the `asset_root` that relative `Asset.path` values resolve
against. Extraction is explicit rather than into a hidden temp directory, so "where are this
document's assets right now" stays answerable. Entry names that would escape the extraction
directory (`..`, absolute paths, drive prefixes) are refused rather than sanitized.

## Block identity

Every content block carries an integer `id`, unique within the document and stable across saves
(spec 0026). Identity is what lets incremental layout cache per block: an index would renumber on
every insert. `revision` is a monotonic edit counter; `next_block_id` is the allocator's state,
persisted so a reload can never hand a deleted block's id to a new one.

All three fields default when absent, so manifests written before they existed still load without a
version bump. Two blocks sharing an id is an error — a duplicate identity would let a cache return
the wrong block's layout.

## What the format deliberately does *not* carry: the POD preset

A **POD preset** (spec 0049 — the bleed, safety margin, ink limit, minimum resolutions, trim
catalogue and PDF/X level one printer states) is an **export-time** concern and is deliberately not
serialized into `.tpub`. A document is not bound to one printer: the same manuscript is routinely
quoted to two, and a preset baked into the manifest would make "which vendor is this book for?" a
property of the file rather than of the export the author is doing right now.

So a preset travels on the command line (`quill preflight --preset …`, `quill export --preset …`),
never in the document. What *is* persisted is the **effect** a preset had at authoring time:
`quill new --preset …` seeds `page_setup.bleed_pt` (and the trim, when the template's own trim is
not one the preset lists) — ordinary page-setup fields that any later export re-checks against
whatever preset it is given. A test asserts a manifest written this way contains no preset.

The presets themselves are code, not data files, and each carries the `source` it was taken from
and the date it was `retrieved`. **Bundled vendor presets are a convenience to be confirmed against
the vendor's current specification, not a warranty**; `quill presets` prints that provenance, and a
preset whose numbers were not read from the vendor is marked unconfirmed and says so when selected.

## Two linked representations

The model carries both a **semantic content** tree (the easy authoring layer) and a **layout**
(master pages, the per-page assignment list, frames and threads — the pro layer); frames reference
content, so editing content reflows layout. See `CLAUDE.md` and the roadmap for how these interact.

("Spreads" appeared in earlier drafts of this document as the layout container. No such type was
ever built: the layout layer is `master_pages` + `pages`, described above. Facing-page behavior
lives in `page_setup.facing_pages` and in the inside/outside margins, not in a spread object.)

## Appendix: the authoring syntax (spec 0043)

A small line-oriented syntax that imports to a document. Deliberately a **subset**, not CommonMark:
the constructs below are all of it, and everything else is an explicit non-goal. Round-tripping back
out to this syntax is also a non-goal.

- `#` … `######` followed by a space — a heading of that level. `#1` is a word, not a heading.
- Blank-line-separated runs of text — a body paragraph; newlines inside one are soft.
- `**bold**` / `__bold__` and `*italic*` / `_italic_` inside a heading or a paragraph (spec 0064) —
  a run set in the family's bold or italic face. They nest: `***both***` is bold italic, and
  `**bold with *italic* inside**` is what it reads as. An **unmatched** delimiter is literal, so
  `2*d6` and `a_b_c` are the text they look like: an opener must be followed by text and a closer
  preceded by it, `_` does not pair inside a word, and a pair is always the same character.
- `![alt](asset-id)` — an image block referencing a linked asset by id.
- `:::panel` … `:::` — a titled record of named sections, one `key: value` per line, where `key` is
  `name`, `overview`, `detail`, `action`, `reaction`, or `attr` (whose value is `name = value`).
  `:::statblock` is the same fence under its pre-0062 name and parses identically; both spellings
  are permanent.
- `:::table` … `:::` — pipe-delimited rows; the first is the header, and a `|---|` separator row is
  ignored as markdown furniture.
- `- item` / `* item` / `1. item` — a list item. The ordinal written in the source is ignored:
  markers are derived from document order when the document is laid out, so inserting an item
  renumbers what follows.
- `:::toc` … `:::` — a generated contents list, taking `title:` and `max_level:`.

Input the importer does not understand is **never silently dropped**. An unknown fence (`:::foo`) is
an error, because the author clearly meant a structured object and both guessing and discarding lose
real content. Everything else is kept as body text with a warning naming the line — a paragraph that
came out as plain prose is visible and fixable; one that vanished is not.

```quill-import
# The Ruined Keep

A dank corridor stretches
into darkness.

![a map](map1)

:::panel
name: Astrolabe, mariner's
overview: Cast brass, Iberian, c. 1580.
attr: Diameter = 180 mm
detail: Alidade replaced; limb graduated in single degrees.
:::

:::table
| d20 | Encounter |
|-----|-----------|
| 1-4 | Nothing   |
:::

:::toc
title: Contents
max_level: 2
:::
```

The example above is parsed by a test, so it cannot drift from what the importer accepts.
