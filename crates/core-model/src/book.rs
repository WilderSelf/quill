//! A **book**: several `.tpub` chapters composed into one document — see `specs/0079-book.md` and
//! `docs/format-spec.md` ("Book files").
//!
//! ## Why a book is a new artifact rather than a field on `Document`
//!
//! The alternative was `Document::chapters`, and it was rejected because it changes what
//! `document.json` *is*. The exported `/ID` is a hash of the manifest text, so anything that changes
//! what a document serializes moves every exported byte of every document — and, worse, it makes
//! every consumer of `Document` carry a standing question it can only answer by convention: *am I
//! the whole book, or a chapter of one?* A chapter does not know it is in a book; a book knows what
//! its chapters are.
//!
//! ## Why a book resolves to one `Document`
//!
//! [`Book::compose`] is a pure function from a book manifest plus its chapters' documents to **one**
//! `Document`. Everything downstream — the layout fixpoint, the incremental session, the font-subset
//! collector, geometry preflight, the PDF/X writer, the outline — is then unchanged, which is the
//! whole argument: those are the paths that have been tested against every increment since M0.
//! Teaching four crates a second N-document code path would give the press file a route nothing else
//! in the repository exercises, and press output is the reason the product exists.
//!
//! It does not make a book *cheaper*: the audit's arithmetic (fixpoint iterations × chapters) is the
//! same either way, expressed as passes over one long document rather than as passes over N short
//! ones. What it does is put that cost on the code path `benches/` already measures.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::container::safe_relative_path;
use crate::{
    Block, BlockId, BreakKind, Document, Folio, LoadError, MasterStatic, Metadata, PackRequirement,
    PageBreak, PageSetup, RunSource, Section, Tpub,
};

/// The book-file format version.
///
/// The **fourth** version chain in this workspace, beside [`crate::FORMAT_VERSION`],
/// [`crate::TEMPLATE_VERSION`] and [`crate::PACK_VERSION`], and deliberately gated by a function
/// written arm for arm with the other two — `version.rs` already says why a third differently-shaped
/// gate would be a third thing to get wrong, and a fourth is worse.
///
/// **2** since spec 0080 gave a chapter a [`BookChapter::break_before`]. The bump is the same rule
/// the document chain follows, applied to the book envelope: a v1 build handed a book that says
/// `"break_before": "recto"` drops the key as unknown, opens every chapter on whichever side the
/// flow happened to reach, and says nothing. Refusing the file is loud; setting the book wrongly is
/// not. The migration is a structural no-op — see `migrate_book`.
pub const BOOK_VERSION: u32 = 2;

/// One chapter of a book: a `.tpub` and what the book says about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookChapter {
    /// Path to the chapter's `.tpub`, **relative to the book file**.
    ///
    /// Refused if it is absolute or escapes the book's directory — `safe_relative_path`, the same
    /// rule a `.tpub` entry and a `.qpack` path already get, and for the same reason: a book file is
    /// something a user can receive from someone else.
    pub path: String,
    /// The chapter's name *as a section*: what `{section}` prints in a running head, and what the
    /// synthesised [`Section`] is called.
    ///
    /// Authored on the book rather than read out of the chapter, for [`Section::name`]'s reason: a
    /// running head says "The Ruined Keep" where the chapter opener says "Chapter One: The Ruined
    /// Keep".
    #[serde(default)]
    pub name: String,
    /// How this chapter's pages are numbered (spec 0073), or `None` to carry the count on.
    ///
    /// This is the hook [`Folio::restart_at`] was left for: `Some(n)` is "the offset a chapter
    /// extracted from a larger book needs". Note that a book needs to state **nothing** to be
    /// numbered continuously — see [`Book::compose`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folio: Option<Folio>,
    /// The master applied to this chapter's opening page.
    ///
    /// `None` falls back to the chapter's own positional override for its own page 0, which is what
    /// every document built from a bundled template carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
    /// The page this chapter opens on (spec 0080).
    ///
    /// **Defaults to [`BreakKind::Page`], and that default is the increment's payoff**: a book
    /// composed from chapters reads as a book without stating anything, where before spec 0080 every
    /// chapter began halfway down the page the previous one ended on. `Recto` is the house style a
    /// printed book usually wants and is what "start this section on the next recto" asks for;
    /// `None` declines the break, for a book whose parts genuinely run on.
    ///
    /// It is stated per chapter rather than once for the book because a book's front matter, its
    /// body chapters and its appendices routinely differ — and because the book file is where a
    /// chapter's other *positional* decisions (its folio, its opening master) are already stated.
    ///
    /// The first chapter's break costs nothing whatever it says: page 0 has received no content, and
    /// a break at the top of a page the flow has not written to is a no-op.
    #[serde(default, skip_serializing_if = "is_page_break")]
    pub break_before: BreakKind,
}

fn is_page_break(kind: &BreakKind) -> bool {
    *kind == BreakKind::Page
}

/// A book file (spec 0079): plain JSON naming the chapters that compose into one press file.
///
/// Not a container, for the reason a template file is not one (spec 0053): a book links no assets of
/// its own. Its chapters are `.tpub` containers and they carry theirs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Book {
    pub book_version: u32,
    /// The book's metadata. A book that states no title inherits its first chapter's.
    #[serde(default)]
    pub metadata: Metadata,
    /// Content packs the **book** requires (spec 0056), unioned with its chapters' own.
    ///
    /// This is the whole of "shared styles", and it is deliberately not a second mechanism: a
    /// `.qpack` already carries templates, styles, definitions and assets with mandatory provenance,
    /// and a requirement that does not resolve is a refusal rather than a fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<PackRequirement>,
    /// The chapters, in reading order.
    pub chapters: Vec<BookChapter>,
}

/// Something composition settled that the author might not have intended.
///
/// Reported rather than silent, and typed rather than a string, because each of these is a decision
/// a book made on the author's behalf. None of them is a press failure — a press failure is a
/// refusal, and the refusals are in [`LoadError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookNote {
    /// A chapter's positional master override for one of its *later* pages was dropped.
    ///
    /// It names a position in a document that no longer starts at page 1, and the offset it would
    /// need is derived rather than known. A chapter's override for its own page **0** is not dropped
    /// — it becomes the synthesised section's master, which is spec 0072's finding that index
    /// assignment is the right representation and the wrong authoring surface.
    DroppedPageOverride { chapter: usize, page: usize },
    /// Two chapters define the same master page, style or component **differently**. The first wins.
    ///
    /// An identical redefinition — what every chapter built from one template has — is not reported,
    /// because reporting it would bury the one that matters.
    Conflict {
        chapter: usize,
        what: &'static str,
        name: String,
    },
    /// A chapter's page setup differs from the book's in a field the book is entitled to settle
    /// (margins, baseline grid). Chapter 0's wins. A difference in trim, bleed or facing pages is a
    /// refusal instead — see [`LoadError::BookPageSetup`].
    PageSetupDiffers { chapter: usize },
    /// A chapter with no content anchors no section, so it contributes no name, folio or opener
    /// master. Not an error: an empty chapter is a placeholder, and refusing a book because one is
    /// still empty would be the wrong trade while it is being written.
    EmptyChapter { chapter: usize },
}

impl fmt::Display for BookNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BookNote::DroppedPageOverride { chapter, page } => write!(
                f,
                "chapter {chapter}: the master assigned to its page {page} was dropped — a \
                 positional assignment cannot be placed in a book; anchor it to a section instead"
            ),
            BookNote::Conflict {
                chapter,
                what,
                name,
            } => write!(
                f,
                "chapter {chapter} defines the {what} `{name}` differently from an earlier \
                 chapter; the earlier one governs the book"
            ),
            BookNote::PageSetupDiffers { chapter } => write!(
                f,
                "chapter {chapter}'s margins or baseline grid differ from the first chapter's; \
                 the first chapter's govern the book"
            ),
            BookNote::EmptyChapter { chapter } => {
                write!(f, "chapter {chapter} has no content and anchors no section")
            }
        }
    }
}

/// What [`Book::compose`] produced: one document, and what it had to settle to get there.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposedBook {
    /// The whole book as a single document. Lay it out, export it, open it — it is a `Document` and
    /// nothing downstream knows it came from several.
    pub document: Document,
    /// Zero-based page in the composed document each chapter's first block *would* start on is not
    /// knowable before layout; what is knowable is which block anchors it. This is that block, per
    /// chapter, so a caller can find a chapter in the composed document without guessing.
    ///
    /// `None` for a chapter with no content.
    pub chapter_anchors: Vec<Option<BlockId>>,
    /// Decisions composition made on the author's behalf. Empty for the ordinary case.
    pub notes: Vec<BookNote>,
}

/// A book opened onto the filesystem: the composed document plus the directory its relative asset
/// paths resolve against.
///
/// The parallel of [`crate::OpenedTpub`], which is **unchanged** by this increment — each chapter is
/// still opened by `Tpub::open_into`, into its own subdirectory of the book's extraction root.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedBook {
    pub composed: ComposedBook,
    pub asset_root: PathBuf,
}

/// Where chapter `i`'s payload is extracted, relative to the book's extraction root.
///
/// One function rather than a string built in two places, because [`Book::compose`] rewrites asset
/// paths with it and [`Book::open_into`] extracts with it, and the two disagreeing would be a
/// linked image that silently fails to resolve (spec 0025's recorded failure mode).
pub fn chapter_prefix(chapter: usize) -> String {
    format!("chapters/{chapter}")
}

impl Book {
    /// Serialize the book file.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a book file, refusing one this build does not understand.
    ///
    /// The gate runs on the untyped value before deserialization, exactly as the document, template
    /// and pack gates do.
    pub fn from_json(text: &str) -> Result<Book, LoadError> {
        let mut value: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| LoadError::BookParse(format!("not valid JSON: {e}")))?;
        crate::version::migrate_book(&mut value)?;
        let book: Book = serde_json::from_value(value)
            .map_err(|e| LoadError::BookParse(format!("does not match the schema: {e}")))?;
        book.validate()?;
        Ok(book)
    }

    /// Structural checks that do not need the chapters themselves.
    fn validate(&self) -> Result<(), LoadError> {
        if self.chapters.is_empty() {
            return Err(LoadError::BookParse(
                "a book must name at least one chapter".into(),
            ));
        }
        for chapter in &self.chapters {
            if safe_relative_path(&chapter.path).is_none() {
                return Err(LoadError::BookUnsafePath {
                    path: chapter.path.clone(),
                });
            }
        }
        Ok(())
    }

    /// Read a book file and open every chapter it names, extracting each into its own subdirectory
    /// of `extract_to`, then compose them.
    ///
    /// Each chapter goes through [`Tpub::open_into`] unchanged — `OpenedTpub` is untouched by this
    /// increment. What makes one asset root serve the whole book is that composition rewrites
    /// chapter `i`'s asset paths to sit under [`chapter_prefix`], which is where its payload was
    /// extracted.
    pub fn open_into(path: &Path, extract_to: &Path) -> Result<OpenedBook, LoadError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| LoadError::BookParse(format!("reading '{}': {e}", path.display())))?;
        let book = Book::from_json(&text)?;

        let base = path.parent().unwrap_or(Path::new("."));
        let mut chapters = Vec::with_capacity(book.chapters.len());
        for (i, chapter) in book.chapters.iter().enumerate() {
            let rel =
                safe_relative_path(&chapter.path).ok_or_else(|| LoadError::BookUnsafePath {
                    path: chapter.path.clone(),
                })?;
            let opened = Tpub::open_into(&base.join(rel), &extract_to.join(chapter_prefix(i)))?;
            chapters.push(opened.document);
        }

        let composed = book.compose(&chapters)?;
        Ok(OpenedBook {
            composed,
            asset_root: extract_to.to_path_buf(),
        })
    }

    /// Compose the chapters into one document.
    ///
    /// Pure: no filesystem, no pack resolution, no layout. The composed document carries the union
    /// of the book's and its chapters' `requires`, so the caller resolves and applies packs to it
    /// exactly as it would for any other document (spec 0056) — there is one pack mechanism, not a
    /// book-shaped second one.
    ///
    /// ## Continuous pagination costs nothing to ask for
    ///
    /// The composed document is one page vector, so spec 0073's guaranteed run at page 0 — arabic
    /// from 1 — already numbers every chapter continuously with nothing stated by anyone. A book
    /// that wants roman front matter says so per chapter, and that statement is a [`Section`]
    /// carrying a [`Folio`]. There is **no page-number offset anywhere**, which is what makes a
    /// stale folio unrepresentable: the thing that decides the folio is `doc.sections`, and that has
    /// been in the incremental session's `context_fingerprint` since spec 0072.
    pub fn compose(&self, chapters: &[Document]) -> Result<ComposedBook, LoadError> {
        if chapters.len() != self.chapters.len() {
            return Err(LoadError::BookParse(format!(
                "the book names {} chapters but {} were supplied",
                self.chapters.len(),
                chapters.len()
            )));
        }
        let first = chapters
            .first()
            .ok_or_else(|| LoadError::BookParse("a book must name at least one chapter".into()))?;

        let mut notes: Vec<BookNote> = Vec::new();
        let mut out = Document {
            format_version: crate::FORMAT_VERSION,
            metadata: if self.metadata.title.is_empty() && self.metadata.authors.is_empty() {
                first.metadata.clone()
            } else {
                self.metadata.clone()
            },
            page_setup: first.page_setup,
            content: Vec::new(),
            assets: Vec::new(),
            fonts_embeddable: chapters.iter().all(|c| c.fonts_embeddable),
            revision: 0,
            next_block_id: 0,
            styles: first.styles.clone(),
            master_pages: Vec::new(),
            default_master: None,
            pages: Vec::new(),
            sections: Vec::new(),
            footnotes: Vec::new(),
            breaks: Vec::new(),
            components: BTreeMap::new(),
            requires: self.requires.clone(),
        };

        let mut anchors: Vec<Option<BlockId>> = Vec::with_capacity(chapters.len());
        // The running id offset: chapter `i`'s ids are shifted past every id used before it, so the
        // composed document has no collision and `assign_missing_block_ids` — which refuses
        // duplicates — is the check that proves it.
        let mut id_base: u64 = 0;

        for (i, (entry, chapter)) in self.chapters.iter().zip(chapters).enumerate() {
            check_page_setup(i, &out.page_setup, &chapter.page_setup, &mut notes)?;

            let mut ch = chapter.clone();
            let used = rebase_block_ids(&mut ch, id_base);
            namespace_assets(&mut ch, i);

            // The chapter's opening-page master: what the book says, else the chapter's own
            // positional override for its own page 0 — the one positional entry whose meaning
            // survives translation, because "the chapter's opening page" is exactly what a section
            // anchor expresses (spec 0072).
            let opener = entry
                .master
                .clone()
                .or_else(|| ch.pages.first().and_then(|p| p.master.clone()));
            for (page, over) in ch.pages.iter().enumerate().skip(1) {
                if over.master.is_some() {
                    notes.push(BookNote::DroppedPageOverride { chapter: i, page });
                }
            }

            merge_named(
                i,
                &mut out.master_pages,
                &ch.master_pages,
                "master page",
                &mut notes,
            );
            for (name, style) in &ch.styles.paragraph {
                match out.styles.paragraph.get(name) {
                    Some(existing) if existing != style => notes.push(BookNote::Conflict {
                        chapter: i,
                        what: "paragraph style",
                        name: name.clone(),
                    }),
                    Some(_) => {}
                    None => {
                        out.styles.paragraph.insert(name.clone(), *style);
                    }
                }
            }
            for (name, def) in &ch.components {
                match out.components.get(name) {
                    Some(existing) if existing != def => notes.push(BookNote::Conflict {
                        chapter: i,
                        what: "component definition",
                        name: name.clone(),
                    }),
                    Some(_) => {}
                    None => {
                        out.components.insert(name.clone(), def.clone());
                    }
                }
            }
            if out.default_master.is_none() {
                out.default_master = ch.default_master.clone();
            }
            for requirement in &ch.requires {
                if !out.requires.contains(requirement) {
                    out.requires.push(requirement.clone());
                }
            }

            // The chapter's own sections first, then the book's — so where both land on the same
            // page the book wins, by the "later authored wins" tie-break spec 0072 already defined.
            // Used deliberately: a chapter that restarts its own numbering on its own first page
            // would print a second page 1 inside a book.
            out.sections.append(&mut ch.sections);
            // The chapter's own breaks travel with it, rebased like every other anchor; the book's
            // break for the chapter's opening block is appended after them, so a book that states
            // `recto` wins over a chapter that states `page` for its own first block by the same
            // "later authored wins" tie-break the sections use.
            out.breaks.append(&mut ch.breaks);
            let anchor = ch.content.first().map(|b| b.id());
            match anchor {
                Some(start) => {
                    out.sections.push(Section {
                        name: entry.name.clone(),
                        start,
                        master: opener,
                        folio: entry.folio,
                    });
                    // **The chapter opens a page** (spec 0080) — the residual spec 0079 named, and
                    // the reason a composed book now reads as a book rather than as one long run of
                    // text. Anchored to the same block the section is, because "the chapter's
                    // opening page" is one statement and it should not be expressible two ways.
                    if entry.break_before.is_break() {
                        out.breaks.push(PageBreak {
                            before: start,
                            kind: entry.break_before,
                        });
                    }
                }
                None => notes.push(BookNote::EmptyChapter { chapter: i }),
            }
            anchors.push(anchor);

            out.content.append(&mut ch.content);
            out.assets.append(&mut ch.assets);
            out.footnotes.append(&mut ch.footnotes);
            id_base += used;
        }

        // Last, and by the check that has always done it: a duplicate id here would mean the rebase
        // missed a site, and that is exactly the thing to fail loudly on.
        out.next_block_id = id_base + 1;
        out.assign_missing_block_ids()?;

        Ok(ComposedBook {
            document: out,
            chapter_anchors: anchors,
            notes,
        })
    }
}

/// Refuse a chapter whose press geometry differs; report one whose design settings do.
///
/// The split is the whole point. A press file whose pages are not one trim is not a press file and
/// POD will reject it, so `trim`, `bleed_pt` and `facing_pages` are a refusal. Margins and the
/// baseline grid are design decisions the book is entitled to settle, and a printer is not harmed by
/// either answer, so they are a note.
fn check_page_setup(
    chapter: usize,
    book: &PageSetup,
    ch: &PageSetup,
    notes: &mut Vec<BookNote>,
) -> Result<(), LoadError> {
    for (field, same) in [
        ("trim", ch.trim == book.trim),
        ("bleed", ch.bleed_pt == book.bleed_pt),
        ("facing pages", ch.facing_pages == book.facing_pages),
    ] {
        if !same {
            return Err(LoadError::BookPageSetup { chapter, field });
        }
    }
    if ch.margins != book.margins || ch.baseline_grid != book.baseline_grid {
        notes.push(BookNote::PageSetupDiffers { chapter });
    }
    Ok(())
}

/// Merge `incoming` into `into` by name, first definition winning, reporting a differing one.
fn merge_named<T: PartialEq + Clone + Named>(
    chapter: usize,
    into: &mut Vec<T>,
    incoming: &[T],
    what: &'static str,
    notes: &mut Vec<BookNote>,
) {
    for item in incoming {
        match into.iter().find(|e| e.name() == item.name()) {
            Some(existing) if existing != item => notes.push(BookNote::Conflict {
                chapter,
                what,
                name: item.name().to_string(),
            }),
            Some(_) => {}
            None => into.push(item.clone()),
        }
    }
}

/// Anything merged by name in a book.
trait Named {
    fn name(&self) -> &str;
}

impl Named for crate::MasterPage {
    fn name(&self) -> &str {
        &self.name
    }
}

/// Shift every identity in `doc` past `base`, and report how many ids the chapter used.
///
/// **Every id-carrying site moves by the same offset**, which is what makes a chapter's internal
/// cross-references still mean what they meant: the block's own id, a `Section::start`, a
/// `RunSource::Reference`'s target, a `RunSource::Footnote`'s note, and a `Footnote`'s own id.
///
/// The run half goes through [`RunSource::remap`], an exhaustive match, so a future `RunSource`
/// variant that carries an identity does not compile until it is handled — spec 0074's structural
/// treatment, at the one site in this increment that owes it.
fn rebase_block_ids(doc: &mut Document, base: u64) -> u64 {
    let shift = |id: &mut BlockId| {
        if id.is_assigned() {
            id.0 += base;
        }
    };
    let mut high = 0u64;
    for block in &mut doc.content {
        let mut id = block.id();
        shift(&mut id);
        high = high.max(id.0);
        block.set_id(id);
        for run in block.runs_mut() {
            run.source.remap(&shift);
        }
    }
    for note in &mut doc.footnotes {
        shift(&mut note.id);
        high = high.max(note.id.0);
        for run in &mut note.runs {
            run.source.remap(&shift);
        }
    }
    for section in &mut doc.sections {
        shift(&mut section.start);
    }
    // A break is anchored to a block exactly as a section is, so it shifts with it (spec 0080).
    // Missing this would not be silent — the break would name a block another chapter now owns, and
    // the page would start in the wrong place — but it is the same class of hazard as an unrebased
    // cross-reference, which is why it sits directly beside the sections it mirrors.
    for br in &mut doc.breaks {
        shift(&mut br.before);
    }
    // The chapter's own high-water mark matters as much as the ids in use: an id handed out and then
    // deleted must not be handed out again by the book, for `BlockId`'s stated reason — reusing the
    // id of a deleted block would hand a stale cache entry to a new one.
    high.max(base + doc.next_block_id).saturating_sub(base)
}

/// Namespace chapter `i`'s asset ids and repoint everything that names one.
///
/// Unconditional rather than collision-detecting, so there is one path rather than two. Two chapters
/// that both link `assets/map.png` under the id `map` are the ordinary case, not an error.
fn namespace_assets(doc: &mut Document, chapter: usize) {
    if doc.assets.is_empty() {
        return;
    }
    let rename = |id: &str| format!("{chapter}/{id}");
    let prefix = chapter_prefix(chapter);
    for asset in &mut doc.assets {
        asset.id = rename(&asset.id);
        asset.path = format!("{prefix}/{}", asset.path);
    }
    for block in &mut doc.content {
        if let Block::Image { asset, .. } = block {
            *asset = rename(asset);
        }
    }
    for master in &mut doc.master_pages {
        for static_item in &mut master.statics {
            if let MasterStatic::Image { asset, .. } = static_item {
                *asset = rename(asset);
            }
        }
    }
}

impl RunSource {
    /// Apply `f` to every identity this source names.
    ///
    /// An **exhaustive** match, deliberately: a new variant carrying a `BlockId` does not compile
    /// until it is handled here, which is spec 0074's answer to "the collector hardcoded `{page}`"
    /// applied to the one remapping site a book needs.
    pub fn remap(&mut self, f: &dyn Fn(&mut BlockId)) {
        match self {
            RunSource::Authored => {}
            RunSource::Reference { target } => f(target),
            RunSource::Footnote { note } => f(note),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Run};

    fn chapter(headings: &[&str]) -> Document {
        let mut doc = Document::sample();
        doc.assets.clear();
        doc.content = headings
            .iter()
            .flat_map(|h| {
                [
                    Block::heading(1, *h, Color::Gray { v: 0.0 }),
                    Block::body(
                        "Body text for the chapter, long enough to occupy a line or two.",
                        Color::Gray { v: 0.0 },
                    ),
                ]
            })
            .collect();
        doc.pages.clear();
        doc.sections.clear();
        doc.next_block_id = 0;
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    fn book_of(n: usize) -> Book {
        Book {
            book_version: BOOK_VERSION,
            metadata: Metadata::default(),
            requires: Vec::new(),
            chapters: (0..n)
                .map(|i| BookChapter {
                    path: format!("ch{i}.tpub"),
                    name: format!("Chapter {i}"),
                    folio: None,
                    master: None,
                    break_before: BreakKind::default(),
                })
                .collect(),
        }
    }

    #[test]
    fn block_ids_are_rebased_so_two_chapters_cannot_collide() {
        let a = chapter(&["One"]);
        let b = chapter(&["Two"]);
        // The two chapters really do share ids before composition — otherwise this proves nothing.
        assert_eq!(
            a.content[0].id(),
            b.content[0].id(),
            "the fixture must actually collide"
        );

        let composed = book_of(2).compose(&[a, b]).expect("compose");
        let ids: Vec<u64> = composed.document.content.iter().map(|b| b.id().0).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "ids must be unique: {ids:?}");
    }

    #[test]
    fn a_chapters_own_cross_reference_still_points_at_its_own_block() {
        // The hazard this guards: a book-wide reference map over *unrebased* ids would resolve
        // chapter 1's reference against chapter 0's block whenever they collide — a wrong page
        // number in a press file, silently.
        let a = chapter(&["One"]);
        let mut b = chapter(&["Two"]);
        let target = b.content[0].id();
        let id = b.content[1].id();
        b.content[1] = Block::body_runs(
            vec![Run::plain("see page "), Run::reference(target)],
            Color::Gray { v: 0.0 },
        );
        b.content[1].set_id(id);

        let composed = book_of(2).compose(&[a, b]).expect("compose");
        let referring = composed
            .document
            .content
            .iter()
            .find(|blk| blk.runs().iter().any(|r| r.source.target().is_some()))
            .expect("the reference survives composition");
        let resolved = referring.runs()[1].source.target().expect("a target");
        let heading = composed
            .document
            .content
            .iter()
            .find(|blk| blk.plain_text().as_deref() == Some("Two"))
            .expect("chapter 1's heading");
        assert_eq!(
            resolved,
            heading.id(),
            "a chapter's reference must still name its own block after rebasing"
        );
    }

    #[test]
    fn assets_are_namespaced_so_two_chapters_may_share_an_id() {
        let mut a = chapter(&["One"]);
        let mut b = chapter(&["Two"]);
        for doc in [&mut a, &mut b] {
            doc.assets = vec![crate::Asset {
                id: "map".into(),
                path: "assets/map.png".into(),
                px_w: 100,
                px_h: 100,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }];
            let mut image = Block::image("map");
            image.set_id(doc.new_block_id());
            doc.content.push(image);
        }

        let composed = book_of(2).compose(&[a, b]).expect("compose");
        let ids: Vec<&str> = composed
            .document
            .assets
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, vec!["0/map", "1/map"]);
        let paths: Vec<&str> = composed
            .document
            .assets
            .iter()
            .map(|a| a.path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec!["chapters/0/assets/map.png", "chapters/1/assets/map.png"]
        );
        let referenced: Vec<&str> = composed
            .document
            .content
            .iter()
            .filter_map(|b| match b {
                Block::Image { asset, .. } => Some(asset.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(referenced, vec!["0/map", "1/map"]);
    }

    #[test]
    fn each_chapter_gets_a_section_anchored_to_its_first_block() {
        let composed = book_of(2)
            .compose(&[chapter(&["One"]), chapter(&["Two"])])
            .expect("compose");
        let names: Vec<&str> = composed
            .document
            .sections
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["Chapter 0", "Chapter 1"]);
        for (section, anchor) in composed
            .document
            .sections
            .iter()
            .zip(&composed.chapter_anchors)
        {
            assert_eq!(Some(section.start), *anchor);
        }
    }

    #[test]
    fn a_chapters_opening_page_master_becomes_its_sections_master() {
        let mut a = chapter(&["One"]);
        a.master_pages = vec![crate::MasterPage::plain("opener")];
        a.pages = vec![crate::PageOverride {
            master: Some("opener".into()),
        }];
        let composed = book_of(1).compose(&[a]).expect("compose");
        assert_eq!(
            composed.document.sections[0].master.as_deref(),
            Some("opener"),
            "the one positional entry whose meaning survives translation"
        );
        assert!(composed.document.pages.is_empty());
        assert!(composed.notes.is_empty(), "{:?}", composed.notes);
    }

    #[test]
    fn a_positional_override_for_a_later_page_is_dropped_and_reported() {
        let mut a = chapter(&["One"]);
        a.master_pages = vec![crate::MasterPage::plain("opener")];
        a.pages = vec![
            crate::PageOverride { master: None },
            crate::PageOverride {
                master: Some("opener".into()),
            },
        ];
        let composed = book_of(1).compose(&[a]).expect("compose");
        assert_eq!(
            composed.notes,
            vec![BookNote::DroppedPageOverride {
                chapter: 0,
                page: 1
            }]
        );
    }

    #[test]
    fn a_chapter_of_a_different_trim_is_refused() {
        let a = chapter(&["One"]);
        let mut b = chapter(&["Two"]);
        b.page_setup.trim.w_pt += 36.0;
        let err = book_of(2).compose(&[a, b]).unwrap_err();
        assert!(
            matches!(err, LoadError::BookPageSetup { chapter: 1, field } if field == "trim"),
            "{err}"
        );
    }

    #[test]
    fn a_differing_margin_is_reported_rather_than_refused() {
        let a = chapter(&["One"]);
        let mut b = chapter(&["Two"]);
        b.page_setup.margins.top_pt += 12.0;
        let composed = book_of(2).compose(&[a, b]).expect("compose");
        assert!(composed
            .notes
            .contains(&BookNote::PageSetupDiffers { chapter: 1 }));
        assert_eq!(composed.document.page_setup.margins, a_margins());
        fn a_margins() -> crate::Margins {
            Document::sample().page_setup.margins
        }
    }

    #[test]
    fn requirements_union_and_reach_the_composed_document() {
        let mut a = chapter(&["One"]);
        a.requires = vec![PackRequirement::new("house", "1")];
        let mut b = chapter(&["Two"]);
        b.requires = vec![PackRequirement::new("house", "1")];
        let mut book = book_of(2);
        book.requires = vec![PackRequirement::new("grimdark", "")];
        let composed = book.compose(&[a, b]).expect("compose");
        assert_eq!(
            composed.document.requires,
            vec![
                PackRequirement::new("grimdark", ""),
                PackRequirement::new("house", "1")
            ]
        );
    }

    #[test]
    fn a_book_file_newer_than_this_build_is_refused() {
        let text = format!(
            r#"{{"book_version": {}, "chapters": [{{"path": "a.tpub"}}]}}"#,
            BOOK_VERSION + 1
        );
        let err = Book::from_json(&text).unwrap_err();
        assert!(
            matches!(err, LoadError::UnsupportedBookVersion { found, supported }
                     if found == BOOK_VERSION + 1 && supported == BOOK_VERSION),
            "{err}"
        );
    }

    #[test]
    fn a_chapter_path_that_escapes_the_book_is_refused() {
        for escaping in ["../secret.tpub", "/etc/passwd"] {
            let text = format!(r#"{{"book_version": 1, "chapters": [{{"path": "{escaping}"}}]}}"#);
            assert!(
                matches!(
                    Book::from_json(&text),
                    Err(LoadError::BookUnsafePath { .. })
                ),
                "'{escaping}' should be refused"
            );
        }
    }

    #[test]
    fn opening_a_book_extracts_every_chapter_and_repoints_its_assets() {
        // The load path end to end: two `.tpub`s that both link `assets/map.png` under the id `map`,
        // opened as a book, must both resolve — which is what asset namespacing plus one extraction
        // root buys, and it is the thing that would silently drop an image if the two halves
        // disagreed about where a chapter's payload went.
        let dir = std::env::temp_dir().join(format!("quill-bookopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        let png = b"\x89PNG\r\n\x1a\n-bytes-are-bytes";
        for (i, title) in ["One", "Two"].iter().enumerate() {
            let mut doc = chapter(&[title]);
            doc.assets = vec![crate::Asset {
                id: "map".into(),
                path: "assets/map.png".into(),
                px_w: 10,
                px_h: 10,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }];
            let mut image = Block::image("map");
            image.set_id(doc.new_block_id());
            doc.content.push(image);
            Tpub::write(
                &doc,
                &dir.join(format!("ch{i}.tpub")),
                &[("assets/map.png", png)],
            )
            .expect("write chapter");
        }
        let book = book_of(2);
        std::fs::write(dir.join("book.qbook"), book.to_json().expect("json")).expect("write book");

        let opened = Book::open_into(&dir.join("book.qbook"), &dir.join("opened")).expect("open");
        assert_eq!(opened.composed.document.assets.len(), 2);
        for asset in &opened.composed.document.assets {
            assert!(
                opened.asset_root.join(&asset.path).exists(),
                "asset '{}' must resolve against the book's one root: {}",
                asset.id,
                asset.path
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The payoff of spec 0080, at the composition site**: a chapter opens a page.
    ///
    /// The `Section` says where the chapter is and what master opens it; the `PageBreak` says the
    /// page is the chapter's own. Both anchored to the same block, because "the chapter's opening
    /// page" is one statement.
    #[test]
    fn each_chapter_gets_a_break_anchored_to_the_block_its_section_is() {
        let composed = book_of(3)
            .compose(&[chapter(&["One"]), chapter(&["Two"]), chapter(&["Three"])])
            .expect("compose");
        let breaks: Vec<BlockId> = composed.document.breaks.iter().map(|b| b.before).collect();
        let anchors: Vec<BlockId> = composed.chapter_anchors.iter().flatten().copied().collect();
        assert_eq!(breaks, anchors, "one break per chapter, on its first block");
        assert!(composed
            .document
            .breaks
            .iter()
            .all(|b| b.kind == BreakKind::Page));
        for (section, br) in composed
            .document
            .sections
            .iter()
            .zip(&composed.document.breaks)
        {
            assert_eq!(section.start, br.before);
        }
    }

    #[test]
    fn a_book_may_ask_for_a_recto_opener_or_decline_the_break() {
        let mut book = book_of(3);
        book.chapters[1].break_before = BreakKind::Recto;
        book.chapters[2].break_before = BreakKind::None;
        let composed = book
            .compose(&[chapter(&["One"]), chapter(&["Two"]), chapter(&["Three"])])
            .expect("compose");
        let third = composed.chapter_anchors[2].expect("anchor");
        assert_eq!(
            composed
                .document
                .breaks
                .iter()
                .map(|b| (b.before, b.kind))
                .collect::<Vec<_>>(),
            vec![
                (
                    composed.chapter_anchors[0].expect("anchor"),
                    BreakKind::Page
                ),
                (
                    composed.chapter_anchors[1].expect("anchor"),
                    BreakKind::Recto
                ),
            ],
            "a declined break is not written as a `None` entry, it is not written at all"
        );
        assert!(composed.document.breaks.iter().all(|b| b.before != third));
    }

    /// A chapter's *own* breaks are rebased with its ids, exactly as its sections and its
    /// cross-references are. Missing this would point a break at a block another chapter owns.
    #[test]
    fn a_chapters_own_breaks_are_rebased_with_its_blocks() {
        let mut a = chapter(&["One", "Two"]);
        a.breaks = vec![PageBreak {
            before: a.content[1].id(),
            kind: BreakKind::Verso,
        }];
        let mut b = chapter(&["Three", "Four"]);
        b.breaks = vec![PageBreak {
            before: b.content[1].id(),
            kind: BreakKind::Verso,
        }];
        let composed = book_of(2).compose(&[a, b]).expect("compose");
        let ids: Vec<BlockId> = composed.document.content.iter().map(Block::id).collect();
        let versos: Vec<BlockId> = composed
            .document
            .breaks
            .iter()
            .filter(|br| br.kind == BreakKind::Verso)
            .map(|br| br.before)
            .collect();
        assert_eq!(versos.len(), 2, "one from each chapter");
        assert_ne!(versos[0], versos[1], "and they cannot have collided");
        for id in &versos {
            assert!(
                ids.contains(id),
                "every break must name a block of the book"
            );
        }
        // The second chapter's break names the *second* chapter's second block — index 5 of the
        // composed content, four blocks of chapter one having gone before it — not the first
        // chapter's, which is what an unrebased anchor would have named.
        assert_eq!(versos[1], ids[5]);
        assert_eq!(versos[0], ids[1]);
    }

    /// A book file written before spec 0080 loads, and its chapters get the break the default
    /// states — which is what makes the increment reach books that already exist.
    #[test]
    fn a_v1_book_file_loads_and_its_chapters_open_a_page() {
        let text = r#"{"book_version": 1, "chapters": [
            {"path": "a.tpub", "name": "One"}, {"path": "b.tpub", "name": "Two"}]}"#;
        let book = Book::from_json(text).expect("a v1 book must still load");
        assert_eq!(book.book_version, BOOK_VERSION);
        assert!(book
            .chapters
            .iter()
            .all(|c| c.break_before == BreakKind::Page));
        // …and it re-serializes without the key, so a book file's bytes do not move either.
        let json = book.to_json().expect("serialize");
        assert!(
            !json.contains("break_before"),
            "the default must not reach the file: {json}"
        );
    }

    #[test]
    fn a_book_round_trips_through_json() {
        let mut book = book_of(2);
        book.chapters[0].folio = Some(Folio {
            format: crate::NumberFormat::LowerRoman,
            restart_at: Some(1),
        });
        let text = book.to_json().expect("serialize");
        assert_eq!(Book::from_json(&text).expect("parse"), book);
    }
}
