# 0073 — Folio formats and restart

**Milestone:** M6 · **Status:** implemented

## Why

Spec 0072 gave the model a section and nothing visible to show for it. This is what a section is
*for*: roman front matter, arabic body, a restart at a part opener. It is the first proof that 0072's
anchor works, and it is deliberately small.

## What

`Section` gains an optional `Folio { format, restart_at }`.

- `format` is a `NumberFormat`, defaulting to `Decimal` — what every page was written in before this
  existed.
- `restart_at: Option<u32>`. `None` continues the count from the page before, which is what a section
  that changes only the *format* wants. `Some(1)` is the restart a part opener asks for; `Some(n)` is
  the offset a chapter extracted from a larger book needs, and is the shape spec 0079's book-level
  pagination will read.

**The numeral machinery is reused, not rewritten.** `NumberFormat::{Decimal, LowerAlpha, UpperAlpha,
LowerRoman, UpperRoman}` with `format()`, `roman()` and `alpha()` already existed — spec 0066 built
them for list markers. This increment is wiring. A second roman numeral converter would have been two
answers to what `iv` means.

**A document is a sequence of `FolioRun`s**, each beginning at a page and carrying a format and the
number that page shows. That derived form is computed once per pass from `Document::sections` and the
section start pages 0072 derives, rather than per page, and both the template's resolver and the
contents list read it — so there is one implementation of what a folio says and two ways of reaching
it, not two answers.

### What prints a folio, and what prints an index

The interesting decision in this increment, and it goes both ways.

- **The `{page}` token prints the folio.** That is the whole feature.
- **A contents entry prints the folio**, because a contents list is read by someone about to turn to
  that page, and a list that says `4` for a page printed `iv` sends them to the wrong place.
  `HeadingEntry` gains a `folio` field for it.
- **`/Link` destinations and PDF outline destinations keep the physical page index.** A destination
  is not a printed number — it is a reference to the *n*th page of the file, and a viewer resolves it
  positionally. Making it follow the folio would break every link in a document with roman front
  matter.

The distinction is between what a *reader* is told and what a *machine* is told. Both are page
numbers; they are not the same page number.

## Acceptance criteria

- Roman front matter, arabic body, restarting at the body's first page — asserted on the **placed
  static text**, not on an intermediate.
- A document with no sections prints exactly what it printed before: arabic, one-based. Asserted.
- **A roman folio's characters are in the font subset and draw no `.notdef`.** See below.
- **A folio-only section costs the fixpoint exactly one extra pass — never two.** This criterion was
  first written as "zero extra iterations, because a folio is furniture and consumes no flow space",
  and the test written for it failed. The claim conflated two things. A folio cannot move a **line
  break**, which is true and is why this converges rather than oscillating; but a section's start page
  is not known until its anchor has been *placed*, so pass 1 necessarily prints default folios and
  pass 2 prints the section's. The second pass is resolution, not instability, and no third is needed
  because the corrected folios move nothing. The real invariant is a floor and a ceiling: a document
  with no sections still takes exactly one pass, and adding a folio-only section takes exactly one
  more.

## The known issue this increment was the first to reach

The font-subset collector carried `'0'..='9'` for `{page}` and nothing else.

`{page}` is replaced at layout time, *after* collection runs, so the characters it will become have
to be carried whether or not any appears in the token. That was the whole answer while a folio was
always arabic. **A lower-roman folio draws `i v x l c d m` and an alpha folio draws letters, and a
digit range contains neither** — so page `iv` of the front matter would have printed as four
`.notdef` boxes in a press file with no error anywhere.

This is `CLAUDE.md`'s silent-press-corruption class, and this exact function was caught by it once
already: PR #107 found that a master's static text was never collected at all, and its comment
records that "digits usually survived by accident, because a contents list contributes `0`–`9`".

The collector now asks the document which formats it configures — `Document::folio_formats()`, which
always includes `Decimal` — and carries each format's alphabet. `NumberFormat::alphabet()` derives
that from the format itself rather than hardcoding a table: `3888` is `mmmdccclxxxviii` and contains
all seven roman symbols at once, and one full turn of the bijective base-26 wheel is every letter an
alpha format can write.

**A book that states no folio format therefore carries exactly the digits it always did**, and its
subset — and so its export byte-hash — does not move.

A smaller defect at the same site is fixed with it: the check tested the string literal `"{page}"`
rather than importing `PAGE_TOKEN`, so renaming the constant would have broken digit embedding with
no compile error.

**The test is asserted on the glyphs actually drawn**, not on set membership, because since spec 0068
the writer draws shaped glyphs — and it was confirmed to fail against the reintroduced defect before
being trusted.

## `FORMAT_VERSION`

**6.** Judged against `docs/format-spec.md`'s own rule — would an older build open the document and
*silently lay it out wrongly*?

It would. A v5 build drops `Section::folio`, so a document with roman front matter prints arabic on
every page of it, quietly, and can save that back over the original. The folios are wrong and nothing
says so. That is the same class as 0072's dropped `sections` and 0047's verso gutter.

It is worth being precise about why the "it hangs off a type v5 cannot read anyway" argument fails:
`Section` itself arrived in 0072, so a **v4** build indeed cannot be misled by this. But a **v5**
build reads `sections` perfectly well, ignores the `folio` field inside it, and produces a book whose
front matter is numbered wrongly. The bump is owed to v5, not to v4.

`migrate_5_to_6` is a structural no-op, written out per the chain's convention. `TEMPLATE_VERSION`
stays 1: none of the four types a template embeds changed shape, and a template carries no sections.

## Non-goals

- **`{section}` and `{heading:N}`** — spec 0074. `Section::name` is the string it will print.
- **"Start this section on the next recto"** — named a non-goal by 0072 and still one. It is a forced
  page break, a mechanism the model lacks entirely, and forward-only rather than a fixpoint.
- **A folio *style* per section** (a different face or size for front matter). `MasterStatic::Text`
  already resolves a named style, so this is authored on the master rather than on the section, and
  needs nothing here.
