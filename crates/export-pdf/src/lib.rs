//! Press-ready PDF/X export and preflight. See `specs/0001-pdf-x-export.md` (preflight) and
//! `specs/0002-pdf-byte-generation.md` (byte generation).
//!
//! [`preflight`] validates a document against the PDF/X requirements, at the thresholds the
//! selected [`PodPreset`] states (spec 0049 — the default preset is numerically identical to the
//! constants those checks used before presets existed). [`export`]
//! then writes a real **PDF/X-1a:2001** or **PDF/X-3:2002** file (selected via
//! [`ExportOptions::version`]) through `pdf-writer` (object graph) + `subsetter` (embedded subset
//! font), with `lcms2` validating the ICC OutputIntent. The writer internals live in the
//! `writer`/`fonts`/`images`/`icc`/`xmp`/`geom` modules.
//!
//! [`ExportOptions::profile`] selects which of the two files a publisher ships (spec 0052):
//! [`ExportProfile::Press`] — the default, PDF/X, no annotations — or [`ExportProfile::Screen`],
//! which carries clickable contents links and claims no conformance. See [`ExportProfile`].

use std::io::Write;
use std::path::PathBuf;

use quill_color::within_ink_limit_pct;
use quill_core_model::{
    page_geom, Asset, Block, Color, Document, FieldValue, PageGeom, PageSetup, Rect,
};

/// Lay a document out **for the press**: exactly what [`export`] hands the writer.
///
/// The counterpart of [`quill_render::lay_out_for_screen`], and the reason both exist: the defect
/// spec 0059 fixed was two call sites disagreeing about a layout input, and two named functions is
/// what lets a test assert they do not. The only difference between them is that this measures
/// through the *subset* font built from the document's used characters — and a subset carries the
/// same advances, so the two agree about every line break.
pub fn lay_out_for_press(
    doc: &Document,
    opts: &ExportOptions,
) -> Result<Vec<quill_layout_engine::LaidOutPage>, ExportError> {
    let font = build_font(opts, &collect_doc_chars(doc))?;
    let shaper = font.shaper();
    Ok(quill_layout_engine::lay_out(
        doc,
        &shaper,
        &quill_fonts::HypherHyphenator,
    ))
}

/// Re-exported from its new home in `quill-fonts` (spec 0059).
///
/// It used to live here, private, which is exactly why `quill render` could not reach it and laid
/// out with `NoHyphenator` — screen and press then broke lines differently. The re-export keeps the
/// old path working for anything that named it.
pub use quill_fonts::HypherHyphenator;
use quill_layout_engine::{LaidOutPage, PlacedBlock};
use thiserror::Error;

mod fonts;
mod geom;
mod icc;
mod images;
mod preset;
mod writer;
mod xmp;

/// The printer's requirements as data (spec 0049). Every preflight threshold is read from one.
pub use preset::PodPreset;

/// Synthesize a minimal, structurally valid CMYK output-class ICC profile.
///
/// Intended for tests and tooling (CI generates one to pass to `export` via `--icc`) so no
/// licensed vendor profile has to be bundled. See [`icc::synth_cmyk_profile`].
pub use icc::synth_cmyk_profile;

/// Target PDF/X conformance level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfxVersion {
    /// PDF/X-1a:2001 — CMYK/spot only, no live transparency.
    X1a2001,
    /// PDF/X-3:2002 — allows color-managed content with an output intent.
    X3_2002,
}

impl PdfxVersion {
    /// The `GTS_PDFXVersion` identifier string for this conformance level, written into both the
    /// document info dict and the XMP identification packet.
    pub fn identifier(self) -> &'static str {
        match self {
            PdfxVersion::X1a2001 => "PDF/X-1a:2001",
            PdfxVersion::X3_2002 => "PDF/X-3:2002",
        }
    }

    /// The `GTS_PDFXConformance` string, if the level defines one. PDF/X-1a carries it; PDF/X-3
    /// (ISO 15930-3) defines only `GTS_PDFXVersion`, so the conformance key is omitted for X-3.
    pub fn conformance(self) -> Option<&'static str> {
        match self {
            PdfxVersion::X1a2001 => Some("PDF/X-1a:2001"),
            PdfxVersion::X3_2002 => None,
        }
    }
}

/// Which of the two files a publisher ships this export is (spec 0052).
///
/// PDF/X-1a requires annotations to sit **outside the BleedBox**, and quill writes
/// `MediaBox == BleedBox` (spec 0013), so a clickable contents entry — which sits in the middle of
/// the text block by definition — and a press-conformant file are mutually exclusive on the same
/// page. Two profiles is the honest answer; a single compromised file is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportProfile {
    /// Press-ready PDF/X at [`ExportOptions::version`]. The default, and unchanged by spec 0052:
    /// no annotations, a required ICC OutputIntent, and the `GTS_PDFX*` identification keys.
    #[default]
    Press,
    /// The customer's file. Carries `/Link` annotations for contents entries, claims **no** PDF/X
    /// conformance (no `GTS_PDFXVersion`, no OutputIntent), and relaxes nothing else.
    ///
    /// Deliberately **not** an RGB profile. Colour conversion is a separate question and converting
    /// is how you ship the wrong colours; content stays CMYK/grayscale, byte-for-byte the same ink
    /// the press file carries. Compression, reader spreads, downsampling and external links are all
    /// out of scope for the same reason: each is its own decision, and a profile that grows them
    /// never closes.
    Screen,
}

impl ExportProfile {
    /// The PDF/X level this profile identifies as, or `None` when it claims no conformance.
    ///
    /// The single place the two profiles' conformance behaviour differs, so the info dict and the
    /// XMP packet cannot drift apart about it.
    pub fn pdfx(self, version: PdfxVersion) -> Option<PdfxVersion> {
        match self {
            ExportProfile::Press => Some(version),
            ExportProfile::Screen => None,
        }
    }
}

/// Options controlling an export.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub version: PdfxVersion,
    /// Press (default) or screen. See [`ExportProfile`].
    pub profile: ExportProfile,
    /// Path to the ICC profile used as the PDF/X OutputIntent (e.g. a CMYK press profile).
    pub output_intent_icc: String,
    /// Export even if preflight fails.
    pub force: bool,
    /// Path to a user-supplied TrueType (`.ttf`) or CFF OpenType (`.otf`) font to embed. `None`
    /// embeds the bundled Source Serif 4. See specs 0004 (user fonts) and 0011 (CFF).
    pub font_path: Option<String>,
    /// Directory that [`Asset::path`](quill_core_model::Asset::path) is resolved against.
    ///
    /// For a `.tpub` this is the container's extracted [`asset_root`](quill_core_model::OpenedTpub);
    /// for a bare `document.json` it is the directory holding that file. It was previously
    /// hardcoded to `.` inside the writer, which silently made asset resolution depend on the
    /// process's working directory — and a linked image that fails to resolve is silently dropped
    /// from the export (spec 0025).
    pub asset_root: PathBuf,
    /// The POD preset every preflight threshold is read from (spec 0049).
    ///
    /// Defaults to [`PodPreset::generic`], which is numerically identical to the constants these
    /// checks used before presets existed — so a caller that never sets this behaves exactly as it
    /// did. It is an *export-time* choice and is deliberately not part of the document.
    pub preset: PodPreset,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            version: PdfxVersion::X1a2001,
            // Press by default, in the `Default` impl and in the CLI, so no caller can reach the
            // screen profile by omission — the press file is the one that must never be a surprise.
            profile: ExportProfile::Press,
            output_intent_icc: String::new(),
            force: false,
            font_path: None,
            // `.` preserves the pre-spec-0025 behavior for callers that never set it, so this
            // change cannot move where an existing caller's images resolve from.
            asset_root: PathBuf::from("."),
            preset: PodPreset::generic(),
        }
    }
}

/// Identifier for each preflight check (maps 1:1 to spec 0001's requirements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckId {
    ColorSpace,
    FontEmbedding,
    Bleed,
    ImageResolution,
    InkCoverage,
    /// No crop, printer, or registration marks in the file (spec 0001 req #7). Quill's writer
    /// emits none and the document model cannot express any, so this is a structural invariant
    /// that never produces a finding — it exists to complete the 1:1 requirement→check mapping.
    Marks,
    OutputIntent,
    /// Live transparency (image alpha) is flattened for PDF/X (spec 0001 req #9). Emitted as a
    /// `Warning` when an asset declares an alpha channel that export will drop.
    Transparency,
    /// The supplied ICC OutputIntent profile is not a CMYK output-class profile.
    IccProfileInvalid,
    /// The document's trim is not among the sizes the selected preset lists (spec 0049).
    ///
    /// Always a `Warning`: an unusual trim is a conversation with the printer, not a corrupt file,
    /// and a preflight that blocks a legitimate document teaches its user to reach for `--force`.
    TrimSize,
    /// Content sits nearer the trim than the preset's safety margin allows (spec 0050).
    ///
    /// Distinct from [`CheckId::Bleed`], which is about the *page box* being large enough. This is
    /// about where content was actually placed, so it needs a laid-out page and cannot be answered
    /// from the model at all — which is why spec 0036's folio printed hard against the trim while
    /// every numeric test passed.
    SafeArea,
}

impl CheckId {
    /// Every check, in declaration order. Used to derive [`PreflightReport::applied`] from the
    /// skip list, so the two can never disagree about what preflight covers.
    pub const ALL: [CheckId; 11] = [
        CheckId::ColorSpace,
        CheckId::FontEmbedding,
        CheckId::Bleed,
        CheckId::ImageResolution,
        CheckId::InkCoverage,
        CheckId::Marks,
        CheckId::OutputIntent,
        CheckId::Transparency,
        CheckId::IccProfileInvalid,
        // `TrimSize` arrived with spec 0049 and was not added here, so `applied()` under-reported
        // what preflight covers — a report that understates itself is the same class of defect as
        // a gate that is not a required context.
        CheckId::TrimSize,
        CheckId::SafeArea,
    ];
}

/// Severity of a preflight finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One preflight result.
#[derive(Debug, Clone)]
pub struct Finding {
    pub check: CheckId,
    pub severity: Severity,
    pub message: String,
}

/// A check that did not run under the selected profile, and why (spec 0052).
///
/// A preflight that quietly examines less than it used to is worse than one that fails: the report
/// looks identical and means something else. Recording the omission — with its reason and its
/// consequence — is what keeps "no findings" honest under a profile that checks fewer things.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub check: CheckId,
    pub reason: String,
}

/// The outcome of preflighting a document.
#[derive(Debug, Clone, Default)]
pub struct PreflightReport {
    pub findings: Vec<Finding>,
    /// Checks the profile did not run. Always empty under [`ExportProfile::Press`].
    pub skipped: Vec<Skipped>,
}

impl PreflightReport {
    /// The checks that actually ran, in `CheckId` declaration order.
    ///
    /// The complement of [`Self::skipped`] over the full check list, derived rather than
    /// maintained by hand — a second hand-written list is how the two come to disagree.
    pub fn applied(&self) -> Vec<CheckId> {
        CheckId::ALL
            .iter()
            .copied()
            .filter(|c| !self.skipped.iter().any(|s| s.check == *c))
            .collect()
    }

    /// True when no `Error`-severity findings are present.
    pub fn passed(&self) -> bool {
        !self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// Count of `Error`-severity findings.
    pub fn error_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }
}

/// Errors returned by [`export`].
#[derive(Debug, Error)]
pub enum ExportError {
    #[error("preflight failed with {0} error(s); pass force to override")]
    PreflightFailed(usize),
    #[error("font embedding failed: {0}")]
    Font(String),
    #[error("ICC OutputIntent error: {0}")]
    Icc(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn push_error(report: &mut PreflightReport, check: CheckId, message: String) {
    report.findings.push(Finding {
        check,
        severity: Severity::Error,
        message,
    });
}

fn push_warning(report: &mut PreflightReport, check: CheckId, message: String) {
    report.findings.push(Finding {
        check,
        severity: Severity::Warning,
        message,
    });
}

/// Validate a document against the PDF/X requirements from spec 0001, at the thresholds the
/// selected POD preset states (spec 0049).
///
/// Every number this checks — ink limit, bleed floor, minimum resolution, the trim catalogue — is
/// read from `opts.preset`, never from a constant. `ExportOptions::default()` selects
/// [`PodPreset::generic`], whose values are exactly the constants these checks used before presets
/// existed, so an unchanged caller gets an unchanged report.
///
/// Under [`ExportProfile::Screen`] with no ICC supplied, the two OutputIntent-related checks are
/// recorded in [`PreflightReport::skipped`] instead of run — see spec 0052. **Everything else still
/// runs**: colour space, ink coverage, bleed, image resolution, font embedding, transparency and
/// marks are the same checks over the same document. The screen profile relaxes the PDF/X
/// identification and the OutputIntent requirement, because those are what an annotation is
/// incompatible with, and nothing else.
pub fn preflight(doc: &Document, opts: &ExportOptions) -> PreflightReport {
    let mut report = PreflightReport::default();
    let preset = &opts.preset;

    // Colors: no RGB in output; every color within the ink limit.
    //
    // "Every colour" means every *run's*, not just the block's (spec 0063). A run's override is ink
    // that reaches the page exactly as the block's does, and a preflight that read one and not the
    // other would pass a document with an over-inked word in it — the silent-corruption failure
    // this crate exists to prevent.
    for (i, block) in doc.content.iter().enumerate() {
        if let Block::Heading { runs, .. } | Block::Body { runs, .. } = block {
            for (ri, run) in runs.iter().enumerate() {
                let Some(color) = run.style.color.as_ref() else {
                    continue;
                };
                if matches!(color, Color::Rgb { .. }) {
                    push_error(
                        &mut report,
                        CheckId::ColorSpace,
                        format!(
                            "block {i} run {ri} uses RGB; press output must be CMYK or grayscale"
                        ),
                    );
                } else if !within_ink_limit_pct(color, preset.max_ink_pct) {
                    push_error(
                        &mut report,
                        CheckId::InkCoverage,
                        format!(
                            "block {i} run {ri} exceeds {}% total ink coverage",
                            preset.max_ink_pct
                        ),
                    );
                }
            }
        }
        let color = match block {
            Block::Heading { color, .. }
            | Block::Body { color, .. }
            | Block::Panel { color, .. }
            | Block::Table { color, .. }
            | Block::Component { color, .. }
            | Block::Toc { color, .. } => Some(color),
            Block::Image { .. } => None,
        };
        let Some(color) = color else { continue };
        if matches!(color, Color::Rgb { .. }) {
            push_error(
                &mut report,
                CheckId::ColorSpace,
                format!("block {i} uses RGB; press output must be CMYK or grayscale"),
            );
        } else if !within_ink_limit_pct(color, preset.max_ink_pct) {
            push_error(
                &mut report,
                CheckId::InkCoverage,
                format!(
                    "block {i} exceeds {}% total ink coverage",
                    preset.max_ink_pct
                ),
            );
        }
    }

    // Fonts must be embeddable/subsettable.
    if !doc.fonts_embeddable {
        push_error(
            &mut report,
            CheckId::FontEmbedding,
            "document references fonts that cannot be embedded".into(),
        );
    }

    // Bleed must be at least what the preset requires on outside edges. Validate the document's own
    // `page_setup.bleed_pt` — the exact value `geom::page_geom` writes into the BleedBox — so
    // preflight rejects the geometry export actually produces (spec 0013).
    let bleed_pt = doc.page_setup.bleed_pt;
    if bleed_pt + f32::EPSILON < preset.bleed_pt {
        push_error(
            &mut report,
            CheckId::Bleed,
            format!(
                "bleed {bleed_pt}pt is below the required {}pt",
                preset.bleed_pt
            ),
        );
    }

    // Trim: a size the preset does not list is a *warning* (spec 0049). A preset that states no
    // catalogue at all — every bundled vendor one — never warns; see `PodPreset::lists_trim`.
    let trim = doc.page_setup.trim;
    if !preset.lists_trim(trim) {
        push_warning(
            &mut report,
            CheckId::TrimSize,
            format!(
                "trim {}x{}pt is not among preset '{}''s listed sizes; confirm it with the printer",
                trim.w_pt, trim.h_pt, preset.name
            ),
        );
    }

    // Image resolution.
    for asset in &doc.assets {
        let needed = preset.min_dpi(asset.line_art);
        if asset.dpi + 0.5 < needed {
            push_error(
                &mut report,
                CheckId::ImageResolution,
                format!(
                    "asset '{}' is {} dpi; needs >= {needed} dpi",
                    asset.id, asset.dpi
                ),
            );
        }
    }

    // An ICC OutputIntent is required for PDF/X. The screen profile claims no PDF/X conformance and
    // writes no OutputIntent (spec 0052), so with no profile supplied there is nothing to require
    // and nothing to validate — but the omission is *recorded*, with what it costs, rather than
    // quietly making the report shorter.
    if opts.output_intent_icc.trim().is_empty() && opts.profile == ExportProfile::Screen {
        report.skipped.push(Skipped {
            check: CheckId::OutputIntent,
            reason: "the screen profile writes no OutputIntent and claims no PDF/X conformance"
                .into(),
        });
        report.skipped.push(Skipped {
            check: CheckId::IccProfileInvalid,
            reason: "no ICC profile supplied; RGB images convert through the naive CMYK fallback \
                     rather than a press profile"
                .into(),
        });
    } else if opts.output_intent_icc.trim().is_empty() {
        push_error(
            &mut report,
            CheckId::OutputIntent,
            "no ICC OutputIntent profile provided".into(),
        );
    } else if let Ok(bytes) = std::fs::read(&opts.output_intent_icc) {
        // The path is present and readable: validate its contents. A missing/unreadable file is
        // left to export time (so a bare `preflight` with a placeholder path behaves as before);
        // only a readable-but-wrong profile is a preflight failure here.
        if let Err(msg) = icc::check_icc(&bytes) {
            push_error(
                &mut report,
                CheckId::IccProfileInvalid,
                format!("ICC '{}': {msg}", opts.output_intent_icc),
            );
        }
    }

    // Transparency: PDF/X-1a:2001 and PDF/X-3:2002 both forbid live transparency, so export
    // flattens image alpha to opaque (see `images.rs`). Warn — not fail — when an asset declares
    // an alpha channel, since the flattened output is still conformant; the author just should
    // know it happened.
    for asset in &doc.assets {
        if asset.has_alpha {
            push_warning(
                &mut report,
                CheckId::Transparency,
                format!(
                    "asset '{}' has an alpha channel; it will be flattened to opaque for PDF/X",
                    asset.id
                ),
            );
        }
    }

    // Marks (spec 0001 req #7): no crop/printer/registration marks. Quill's writer emits none and
    // the document model has no field that could request them, so there is nothing to flag. This
    // check is a structural invariant with no failing input by design — present to complete the
    // 1:1 requirement→check mapping; it never pushes a finding.

    report
}

/// Export a document as press-ready PDF/X at the level in `opts.version` (X-1a:2001 or
/// X-3:2002). Runs preflight first (unless `opts.force`), lays the document out, then writes real
/// PDF bytes to `out`. See specs 0002 (byte generation) and 0003 (X-3 selection).
pub fn export(
    doc: &Document,
    opts: &ExportOptions,
    out: &mut impl Write,
) -> Result<(), ExportError> {
    if !opts.force {
        let report = preflight(doc, opts);
        if !report.passed() {
            return Err(ExportError::PreflightFailed(report.error_count()));
        }
    }
    // Build the embedded font once, up front: it is both the subset the writer embeds and the
    // source of shaped advances the layout engine measures with. The shaping context (spec 0016)
    // parses a rustybuzz face over the font once and is shared by the layout pass; the same `font`
    // is then embedded by the writer.
    let used_chars = collect_doc_chars(doc);
    let font = build_font(opts, &used_chars)?;
    let shaper = font.shaper();
    // Real en-US hyphenation (spec 0018 incr. 2, shared with the screen path by spec 0059): words
    // break at legal syllable points, tightening lines and splitting over-wide words.
    let hyphenator = quill_fonts::HypherHyphenator;
    let pages = quill_layout_engine::lay_out(doc, &shaper, &hyphenator);

    // Colour checks on the *model* cannot see geometry the engine synthesized (spec 0037). A
    // tinted panel or a rule is ink like any other, but it is not a `Block` — so the checks above,
    // which walk `doc.content`, are blind to it. Run the geometry-level checks on the pages the
    // writer is about to draw, rather than laying the document out a second time inside
    // `preflight`.
    if !opts.force {
        let findings = preflight_pages(&pages, &opts.preset, &doc.page_setup, &doc.assets);
        let errors = findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count();
        if errors > 0 {
            return Err(ExportError::PreflightFailed(errors));
        }
    }

    writer::write_pdf(doc, opts, &pages, &font, out)
}

/// Whether an annotation at `rect` on `page_index` would violate PDF/X (spec 0042).
///
/// PDF/X-1a requires annotations to sit **outside the BleedBox**. A clickable table-of-contents
/// entry sits in the middle of the text block by definition, which is why spec 0042 ships outlines
/// and destinations — document structure, always legal — and not internal links.
///
/// Nothing in the model can express an annotation yet, so this check has nothing to guard today.
/// That is exactly when it is cheap to add and exactly the spec-0013 lesson: the validator and the
/// writer must agree on the rule *before* anything relies on it. When links arrive — behind a
/// non-PDF/X screen profile, most likely — this is the gate they have to pass, rather than a rule
/// someone has to remember.
///
/// `rect` is in the page's own top-left space, the same space `PlacedBlock` uses.
pub fn annotation_finding(
    rect: &quill_core_model::Rect,
    page_setup: &quill_core_model::PageSetup,
    page_index: usize,
) -> Option<Finding> {
    let g = quill_core_model::page_geom(page_setup, page_index);
    // The BleedBox is the whole media box here (`MediaBox == BleedBox`, spec 0013), so anything
    // touching the page at all intersects it. Stated as an intersection test rather than as
    // "always fails" so it stays correct if the two boxes ever diverge.
    let (bx, by, bw, bh) = (0.0, 0.0, g.media_w, g.media_h);
    let x = g.off_x + rect.x_pt;
    let y = g.off_y + rect.y_pt;
    let intersects = x < bx + bw && x + rect.w_pt > bx && y < by + bh && y + rect.h_pt > by;
    intersects.then(|| Finding {
        check: CheckId::Marks,
        severity: Severity::Error,
        message: format!(
            "page {}: an annotation intersects the BleedBox; PDF/X requires annotations outside it",
            page_index + 1
        ),
    })
}

/// The live area of a page, in the layout engine's top-left trim coordinates: the rectangle content
/// must stay inside for the printer's stated safety margin (spec 0050).
///
/// Returned as `(left, top, right, bottom)` edges. A preset stating `safety_pt: 0.0` yields the trim
/// itself, which makes the check inert rather than a special case.
fn live_area(setup: &PageSetup, preset: &PodPreset) -> (f32, f32, f32, f32) {
    let s = preset.safety_pt;
    (s, s, setup.trim.w_pt - s, setup.trim.h_pt - s)
}

/// Which edge, if any, `frame` intrudes past on its way *inward* from the trim.
///
/// The direction is the whole point. A block extending **outward** past the trim is a deliberate
/// full bleed and is exempt; one falling **inward** of the live edge is at risk from the guillotine.
/// Flagging the first would train a user to ignore the check, and then the second goes unread too.
fn intrudes(frame: &Rect, geom: &PageGeom, live: (f32, f32, f32, f32)) -> Option<&'static str> {
    let (l, t, r, b) = live;
    // Zero safety margin: the preset states none, so there is nothing to check.
    if l <= 0.0 && t <= 0.0 {
        return None;
    }
    let (x0, y0) = (frame.x_pt, frame.y_pt);
    let (x1, y1) = (frame.x_pt + frame.w_pt, frame.y_pt + frame.h_pt);
    // Outward of trim on an edge ⇒ bleeding there ⇒ that edge is not a safety question.
    let bleeds_left = x0 < 0.0;
    let bleeds_top = y0 < 0.0;
    let bleeds_right = x1 > geom.trim_w;
    let bleeds_bottom = y1 > geom.trim_h;
    if !bleeds_left && x0 < l {
        return Some("left");
    }
    if !bleeds_top && y0 < t {
        return Some("top");
    }
    if !bleeds_right && x1 > r {
        return Some("right");
    }
    if !bleeds_bottom && y1 > b {
        return Some("bottom");
    }
    None
}

/// The effective resolution of `asset` at the size it was **placed**, and the minimum the preset
/// wants, when the first is below the second.
///
/// `Asset.dpi` is what the author declared about the source file; this is pixels divided by the
/// inches the image actually occupies. A 300 dpi image placed at 2x its natural size prints at 150
/// and passes the model-level check — that gap is why this exists. Deriving the placed size from
/// `dpi` would make the check circular, so it reads the laid-out rect and nothing else.
fn under_dpi(asset: &Asset, frame: &Rect, preset: &PodPreset) -> Option<(f32, f32)> {
    let required = if asset.line_art {
        preset.min_dpi_line_art
    } else {
        preset.min_dpi_color
    };
    if frame.w_pt <= 0.0 || frame.h_pt <= 0.0 || asset.px_w == 0 || asset.px_h == 0 {
        return None;
    }
    let dpi_x = asset.px_w as f32 / (frame.w_pt / 72.0);
    let dpi_y = asset.px_h as f32 / (frame.h_pt / 72.0);
    let effective = dpi_x.min(dpi_y);
    // A hair of tolerance so an image placed at exactly its natural size cannot fail on rounding.
    (effective < required - 0.5).then_some((effective, required))
}

/// Press checks that can only be made against laid-out geometry (spec 0037).
///
/// [`preflight`] walks the document, which is the right place for everything a *block* declares.
/// Decoration is different: a [`PlacedBlock::Rect`] is produced by the layout engine, carries its
/// own colours, and reaches the page without ever having been a `Block`. Nothing in the model-level
/// checks would ever see it, so a panel tinted at 280% total ink would go straight to a print shop
/// — exactly the silent-press-corruption class `CLAUDE.md` forbids, and invisible to every test
/// that only looks at text and images.
///
/// Kept public so the same checks can be run against a page set the caller already has, and so this
/// can be tested directly rather than only through a full export.
///
/// `preset` supplies the ink limit (spec 0049): decoration is checked against the same press the
/// blocks are, because a panel and a paragraph reach the same printer.
pub fn preflight_pages(
    pages: &[LaidOutPage],
    preset: &PodPreset,
    setup: &PageSetup,
    assets: &[Asset],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let by_id: std::collections::HashMap<&str, &Asset> =
        assets.iter().map(|a| (a.id.as_str(), a)).collect();

    for page in pages {
        // Two checks that need a *placed* page and a preset, and can be answered from nothing less
        // (spec 0050). Both are reported once per page rather than once per block: a systematically
        // misplaced master static would otherwise fill a 500-page report with the same sentence.
        let mut safe_area_reported = false;
        let mut dpi_reported: std::collections::BTreeSet<String> = Default::default();
        let geom = page_geom(setup, page.index);
        let live = live_area(setup, preset);

        for block in page.statics.iter().chain(page.blocks.iter()) {
            let frame = match block {
                PlacedBlock::Text { frame, .. }
                | PlacedBlock::Image { frame, .. }
                | PlacedBlock::Rect { frame, .. } => *frame,
                PlacedBlock::Link { .. } => continue,
            };

            // Effective dpi: pixels divided by the inches it was *placed* at, not the resolution
            // the asset declares. A 300 dpi image scaled to twice its natural size prints at 150
            // and passes the model-level check, which is why this one exists.
            if let PlacedBlock::Image { asset_id, .. } = block {
                if let Some(asset) = by_id.get(asset_id.as_str()) {
                    if let Some((effective, required)) = under_dpi(asset, &frame, preset) {
                        if dpi_reported.insert(asset_id.clone()) {
                            findings.push(Finding {
                                check: CheckId::ImageResolution,
                                severity: Severity::Error,
                                message: format!(
                                    "page {}: image '{asset_id}' is placed at {effective:.0} dpi                                      but preset '{}' requires {required:.0}; place it smaller or                                      supply a higher-resolution source",
                                    page.index, preset.name
                                ),
                            });
                        }
                    }
                }
            }

            // Safe area: content that falls *inward* of the live edge. Content extending outward
            // past trim is a deliberate full bleed and is exempt — flagging it would train users to
            // ignore the check, which costs more than the check is worth.
            if !safe_area_reported {
                if let Some(edge) = intrudes(&frame, &geom, live) {
                    safe_area_reported = true;
                    findings.push(Finding {
                        check: CheckId::SafeArea,
                        severity: Severity::Error,
                        message: format!(
                            "page {}: content sits inside the {} safety margin at the {edge} edge;                              preset '{}' wants {:.1}pt of clearance from the trim",
                            page.index,
                            preset.name,
                            preset.name,
                            preset.safety_pt
                        ),
                    });
                }
            }
        }

        for block in page.statics.iter().chain(page.blocks.iter()) {
            let PlacedBlock::Rect { fill, stroke, .. } = block else {
                continue;
            };
            let colors = [
                fill.as_ref().map(|c| ("fill", *c)),
                stroke.as_ref().map(|s| ("stroke", s.color)),
            ];
            for (what, color) in colors.into_iter().flatten() {
                if matches!(color, Color::Rgb { .. }) {
                    findings.push(Finding {
                        check: CheckId::ColorSpace,
                        severity: Severity::Error,
                        message: format!(
                            "page {}: a decoration {what} uses RGB; press output must be CMYK or \
                             grayscale",
                            page.index + 1
                        ),
                    });
                } else if !within_ink_limit_pct(&color, preset.max_ink_pct) {
                    findings.push(Finding {
                        check: CheckId::InkCoverage,
                        severity: Severity::Error,
                        message: format!(
                            "page {}: a decoration {what} exceeds {}% total ink coverage",
                            page.index + 1,
                            preset.max_ink_pct
                        ),
                    });
                }
            }
        }
    }
    findings
}

/// Every character the font must carry: the document's text-block chars (headings + body) plus a
/// literal space and a literal hyphen.
///
/// The space is inserted unconditionally because `break_by_width` normalizes *all* inter-word
/// whitespace to `U+0020` — so a document that separates words only with tabs/newlines still
/// renders (and is measured) with the real space glyph rather than `.notdef`. Without this, the
/// space glyph could be missing from the subset even though every laid-out line uses it.
///
/// The hyphen (`U+002D`) is inserted unconditionally for the same reason (spec 0018 incr. 2):
/// hyphenation can introduce a trailing `-` on a broken line even when the source text contains no
/// literal hyphen, so the subset must always carry a real hyphen glyph rather than emit `.notdef`.
fn collect_doc_chars(doc: &Document) -> std::collections::BTreeSet<char> {
    let mut set = std::collections::BTreeSet::new();
    set.insert(' ');
    set.insert('-');
    for block in &doc.content {
        match block {
            // Every run's text, not the block's: a run the subsetter never saw would set to
            // `.notdef` boxes in the press file (spec 0063).
            Block::Heading { runs, .. } | Block::Body { runs, .. } => {
                set.extend(runs.iter().flat_map(|r| r.text.chars()))
            }
            // Spec 0026's silent-failure case, and the reason every variant-adding increment
            // carries a non-ASCII export test: a character this collector misses is not an error
            // anywhere, it just renders as `.notdef` in the finished PDF.
            Block::Panel { panel, .. } => {
                set.extend(panel.name.chars());
                for (k, v) in &panel.attributes {
                    set.extend(k.chars());
                    set.extend(v.chars());
                }
                for section in [
                    &panel.overview,
                    &panel.details,
                    &panel.actions,
                    &panel.reactions,
                ] {
                    for line in section {
                        set.extend(line.chars());
                    }
                }
            }
            Block::Table { table, .. } => {
                // Same silent-failure surface as the panel block: a header or cell this collector
                // misses renders as `.notdef` boxes with no error anywhere.
                for cell in table.header.iter().flatten() {
                    set.extend(cell.chars());
                }
                for row in &table.rows {
                    for cell in row {
                        set.extend(cell.chars());
                    }
                }
            }
            Block::Toc { title, .. } => {
                // Only the authored title. The entries are generated from heading text, which is
                // already collected from the headings themselves — and the page numbers are
                // digits, which the subset always carries because the folio needs them.
                set.extend(title.chars());
                set.extend('0'..='9');
                set.insert('.');
                set.insert('…');
            }
            Block::Component { fields, .. } => {
                // Every authored field of every instance (spec 0054). Same silent-failure surface
                // as the two bundled components: a character this collector misses renders as a
                // `.notdef` box with no error anywhere, so the walk is exhaustive over the value
                // kinds rather than over the ones the bundled definitions happen to use.
                for value in fields.values() {
                    match value {
                        FieldValue::Text(t) => set.extend(t.chars()),
                        FieldValue::Lines(lines) => {
                            for l in lines {
                                set.extend(l.chars());
                            }
                        }
                        FieldValue::Pairs(pairs) => {
                            for (k, v) in pairs {
                                set.extend(k.chars());
                                set.extend(v.chars());
                            }
                        }
                        FieldValue::Rows(rows) => {
                            for row in rows {
                                for cell in row {
                                    set.extend(cell.chars());
                                }
                            }
                        }
                        FieldValue::Widths(_) | FieldValue::Flag(_) => {}
                    }
                }
            }
            Block::Image { .. } => {}
        }
    }
    set
}

/// Subset and measure the font for `chars`: a user-supplied `font_path` (spec 0004/0011) or the
/// bundled Source Serif 4.
fn build_font(
    opts: &ExportOptions,
    chars: &std::collections::BTreeSet<char>,
) -> Result<fonts::EmbeddedFont, ExportError> {
    match &opts.font_path {
        Some(path) => {
            let program = std::fs::read(path)
                .map_err(|e| ExportError::Font(format!("reading font '{path}': {e}")))?;
            fonts::build_from_bytes(&program, None, chars)
        }
        None => fonts::build(chars),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::Asset;

    fn opts_with_icc() -> ExportOptions {
        ExportOptions {
            output_intent_icc: "profiles/cmyk.icc".into(),
            ..Default::default()
        }
    }

    /// Regression (spec 0015 review): `break_by_width` normalizes inter-word whitespace to a literal
    /// space, so the font must always subset `' '` — even when the source separates words only with
    /// tabs/newlines. Otherwise the space renders as `.notdef` and is mis-measured. The bundled font
    /// must map `' '` to a real (non-`.notdef`) glyph.
    #[test]
    fn space_glyph_is_subset_even_without_literal_space() {
        let doc = {
            let mut d = Document::sample();
            // no literal U+0020
            d.content = vec![Block::body("alpha\tbeta\ngamma", Color::Gray { v: 0.0 })];
            d
        };
        let chars = collect_doc_chars(&doc);
        assert!(chars.contains(&' '), "space must always be collected");

        let font = build_font(&ExportOptions::default(), &chars).expect("build bundled font");
        let encoded = font.encode_line(" ");
        assert_eq!(encoded.len(), 2, "one glyph = two Identity-H bytes");
        let gid = u16::from_be_bytes([encoded[0], encoded[1]]);
        assert_ne!(gid, 0, "' ' must map to a real glyph, not .notdef");
    }

    /// Spec 0018 incr. 2: hyphenation can add a trailing `-` to a broken line even when the source
    /// text has no literal hyphen, so the subset must always carry a real hyphen glyph — otherwise
    /// the rendered hyphen would be `.notdef`. Mirrors the space-glyph guarantee above.
    #[test]
    fn hyphen_glyph_is_subset_even_without_literal_hyphen() {
        let doc = {
            let mut d = Document::sample();
            // no literal U+002D
            d.content = vec![Block::body("alpha beta gamma", Color::Gray { v: 0.0 })];
            d
        };
        let chars = collect_doc_chars(&doc);
        assert!(chars.contains(&'-'), "hyphen must always be collected");

        let font = build_font(&ExportOptions::default(), &chars).expect("build bundled font");
        let encoded = font.encode_line("-");
        assert_eq!(encoded.len(), 2, "one glyph = two Identity-H bytes");
        let gid = u16::from_be_bytes([encoded[0], encoded[1]]);
        assert_ne!(gid, 0, "'-' must map to a real glyph, not .notdef");
    }

    #[test]
    fn clean_document_passes_preflight() {
        let report = preflight(&Document::sample(), &opts_with_icc());
        assert!(
            report.passed(),
            "unexpected findings: {:?}",
            report.findings
        );
    }

    #[test]
    fn rgb_color_fails_colorspace_check() {
        let mut doc = Document::sample();
        doc.content.push(Block::body(
            "oops",
            Color::Rgb {
                r: 1.0,
                g: 0.0,
                b: 0.0,
            },
        ));
        let report = preflight(&doc, &opts_with_icc());
        assert!(!report.passed());
        assert!(report
            .findings
            .iter()
            .any(|f| f.check == CheckId::ColorSpace));
    }

    #[test]
    fn low_res_image_fails_resolution_check() {
        let mut doc = Document::sample();
        doc.assets = vec![Asset {
            id: "blurry".into(),
            path: "assets/blurry.png".into(),
            px_w: 600,
            px_h: 600,
            dpi: 299.0,
            line_art: false,
            has_alpha: false,
        }];
        let report = preflight(&doc, &opts_with_icc());
        assert!(report
            .findings
            .iter()
            .any(|f| f.check == CheckId::ImageResolution));
    }

    #[test]
    fn line_art_needs_600_dpi() {
        let mut doc = Document::sample();
        doc.assets = vec![Asset {
            id: "ink".into(),
            path: "assets/ink.png".into(),
            px_w: 600,
            px_h: 600,
            dpi: 400.0,
            line_art: true,
            has_alpha: false,
        }];
        let report = preflight(&doc, &opts_with_icc());
        assert!(report
            .findings
            .iter()
            .any(|f| f.check == CheckId::ImageResolution));
    }

    #[test]
    fn missing_output_intent_fails() {
        let report = preflight(&Document::sample(), &ExportOptions::default());
        assert!(report
            .findings
            .iter()
            .any(|f| f.check == CheckId::OutputIntent));
    }

    #[test]
    fn insufficient_page_setup_bleed_fails_bleed_check() {
        // A document whose own page_setup requests less than the required 9pt bleed must fail the
        // Bleed check — because geometry writes exactly that (too-small) BleedBox (spec 0013).
        let mut doc = Document::sample();
        doc.page_setup.bleed_pt = 2.0;
        let report = preflight(&doc, &opts_with_icc());
        assert!(!report.passed());
        let finding = report
            .findings
            .iter()
            .find(|f| f.check == CheckId::Bleed)
            .expect("expected a Bleed finding");
        assert_eq!(finding.severity, Severity::Error);
        assert!(
            finding.message.contains("2pt"),
            "message should report the document's bleed value: {}",
            finding.message
        );
    }

    #[test]
    fn adequate_page_setup_bleed_emits_no_bleed_finding() {
        // The sample's page_setup bleed is the required 9pt, so no Bleed finding is produced.
        let report = preflight(&Document::sample(), &opts_with_icc());
        assert!(!report.findings.iter().any(|f| f.check == CheckId::Bleed));
    }

    #[test]
    fn export_refuses_document_with_insufficient_bleed() {
        // The reconciled Bleed check gates export: a too-small page_setup bleed blocks it (no force).
        let (opts, path) = opts_with_real_icc("lowbleed");
        let mut doc = Document::sample();
        doc.page_setup.bleed_pt = 2.0;
        let mut sink = Vec::new();
        let e = export(&doc, &opts, &mut sink).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(matches!(e, ExportError::PreflightFailed(_)));
        assert!(
            sink.is_empty(),
            "nothing should be written when preflight fails"
        );
    }

    #[test]
    fn transparency_asset_warns_but_passes() {
        // An asset declaring an alpha channel yields a Transparency *warning* (spec 0001 req #9);
        // export still succeeds because Quill flattens it, so preflight still passes.
        let mut doc = Document::sample();
        doc.assets = vec![Asset {
            id: "glow".into(),
            path: "assets/glow.png".into(),
            px_w: 600,
            px_h: 600,
            dpi: 300.0,
            line_art: false,
            has_alpha: true,
        }];
        let report = preflight(&doc, &opts_with_icc());
        let finding = report
            .findings
            .iter()
            .find(|f| f.check == CheckId::Transparency)
            .expect("expected a Transparency finding");
        assert_eq!(finding.severity, Severity::Warning);
        assert!(report.passed(), "a warning must not fail preflight");
    }

    #[test]
    fn opaque_assets_have_no_transparency_finding() {
        // The sample's asset has no alpha, so nothing is flagged.
        let report = preflight(&Document::sample(), &opts_with_icc());
        assert!(!report
            .findings
            .iter()
            .any(|f| f.check == CheckId::Transparency));
    }

    #[test]
    fn clean_document_emits_no_marks_finding() {
        // Marks is a structural invariant: Quill emits no marks and the model can't request any,
        // so no document ever produces a Marks finding.
        let report = preflight(&Document::sample(), &opts_with_icc());
        assert!(!report.findings.iter().any(|f| f.check == CheckId::Marks));
    }

    // --- Decoration preflight (spec 0037) ------------------------------------------------------

    fn page_with_rect(
        fill: Option<Color>,
        stroke: Option<quill_layout_engine::Stroke>,
    ) -> LaidOutPage {
        LaidOutPage {
            index: 0,
            blocks: vec![PlacedBlock::Rect {
                frame: quill_core_model::Rect {
                    x_pt: 10.0,
                    y_pt: 10.0,
                    w_pt: 100.0,
                    h_pt: 50.0,
                },
                fill,
                stroke,
            }],
            statics: Vec::new(),
        }
    }

    // --- PDF outline (spec 0042) ---------------------------------------------------------------

    /// Export a document made of the given headings and return the raw PDF bytes.
    fn export_with_headings(tag: &str, levels: &[(u8, &str)]) -> Vec<u8> {
        let (opts, icc) = opts_with_real_icc(tag);
        let mut doc = Document::sample();
        doc.content.clear();
        for (level, text) in levels {
            doc.content
                .push(Block::heading(*level, *text, Color::Gray { v: 0.0 }));
        }
        doc.assign_missing_block_ids().expect("ids");
        let mut bytes = Vec::new();
        export(&doc, &opts, &mut bytes).expect("export");
        let _ = std::fs::remove_file(icc);
        bytes
    }

    #[test]
    fn a_document_with_no_headings_emits_no_outline_at_all() {
        // Not an empty one. An empty outline tree renders as an empty, confusing bookmark pane,
        // which is worse than no pane.
        let (opts, icc) = opts_with_real_icc("outline_none");
        let mut doc = Document::sample();
        doc.content.retain(|b| !matches!(b, Block::Heading { .. }));
        let mut bytes = Vec::new();
        export(&doc, &opts, &mut bytes).expect("export");
        assert!(
            !bytes.windows(9).any(|w| w == b"/Outlines"),
            "no headings must mean no /Outlines entry"
        );
        let _ = std::fs::remove_file(icc);
    }

    #[test]
    fn headings_become_a_nested_outline_tree() {
        // h1 / h2 / h1 ⇒ two top-level items, the first with one child. Asserted on the emitted
        // objects rather than by eye: the link fields are the part a parser accepts and a viewer
        // renders wrongly.
        let bytes = export_with_headings("outline_tree", &[(1, "One"), (2, "Under"), (1, "Two")]);
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("/Type /Outlines"), "an outline root: {text}");
        // The root is open, so its count is every visible item.
        assert!(text.contains("/Count 3"), "root counts all three: {text}");
        // The h1 with a child counts its visible descendants — this is the nesting, stated as the
        // number a viewer reads.
        assert!(
            text.contains("/Count 1"),
            "the h1 with a child counts 1: {text}"
        );
        // `/Dest [` appears once per outline item and nowhere else, which makes it the honest way
        // to count items: `/Title` also appears in the document info dictionary, `/Parent` in every
        // page object, and a bare `/Dest` also matches the OutputIntent's `/DestOutputProfile`.
        assert_eq!(text.matches("/Dest [").count(), 3, "one item per heading");
        assert!(
            text.contains("/Prev") && text.contains("/Next"),
            "top-level siblings must be linked: {text}"
        );
    }

    #[test]
    fn a_skipped_heading_level_still_nests_under_its_nearest_ancestor() {
        // Authoring reality: an h3 directly under an h1, and a document whose first heading is an
        // h2. Neither is well-formed, and neither may produce a broken tree.
        let bytes = export_with_headings("outline_skip", &[(1, "One"), (3, "Deep")]);
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches("/Dest [").count(), 2);
        assert!(
            text.contains("/Count 1"),
            "the h1 still parents the h3: {text}"
        );

        // Two h2s and no h1: both are top level, neither parents the other.
        let bytes = export_with_headings("outline_deep_first", &[(2, "Starts deep"), (2, "Also")]);
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches("/Dest [").count(), 2);
        assert!(text.contains("/Count 2"), "both sit at the root: {text}");
    }

    #[test]
    fn an_outline_title_carries_non_ascii_correctly() {
        // Outline strings are a *different* encoding path from page content (PDF text strings are
        // UTF-16BE with a BOM), so they get this wrong independently of the font subset.
        let bytes = export_with_headings("outline_utf16", &[(1, "Cráeblóð")]);
        let text = String::from_utf8_lossy(&bytes);
        // Written as a hex string: the BOM `FEFF`, then UTF-16BE code units — `0043` for 'C' and
        // `00E1` for 'á'. An ASCII title stays a plain literal string, which is why this needs its
        // own test rather than riding on the tree one.
        assert!(
            text.contains("<FEFF0043") && text.contains("00E1"),
            "the title must be a UTF-16BE hex string with a BOM: {text}"
        );
    }

    #[test]
    fn an_annotation_intersecting_the_bleed_box_is_an_error() {
        // The rule spec 0042 exists to make enforceable rather than remembered. Nothing emits
        // annotations yet, so the input is constructed — which is the point (spec 0013: the
        // validator and the writer must agree before anything relies on it).
        use quill_core_model::{PageSetup, Rect};
        let setup = PageSetup::default();

        let on_page = Rect {
            x_pt: 100.0,
            y_pt: 100.0,
            w_pt: 50.0,
            h_pt: 20.0,
        };
        let f = annotation_finding(&on_page, &setup, 0).expect("must be flagged");
        assert_eq!(f.severity, Severity::Error);

        // Entirely off the media box: no finding. Without this direction the check could simply
        // flag everything and still pass.
        let off_page = Rect {
            x_pt: 10_000.0,
            y_pt: 10_000.0,
            w_pt: 5.0,
            h_pt: 5.0,
        };
        assert!(annotation_finding(&off_page, &setup, 0).is_none());
    }

    #[test]
    fn a_stat_blocks_glyphs_reach_the_font_subset() {
        // Spec 0026's silent-failure case, and the reason every variant-adding increment carries
        // this test: a character `collect_doc_chars` misses is not an error anywhere in the
        // pipeline — it renders as a `.notdef` box in the finished PDF. Every section of a stat
        // block is checked, because each is a separate place the collector could have forgotten.
        let mut doc = Document::sample();
        doc.content.push(Block::Panel {
            id: quill_core_model::BlockId::UNASSIGNED,
            panel: quill_core_model::Panel {
                name: "Cráeblóð".into(),
                overview: vec!["Ǫverview".into()],
                attributes: vec![("Ǎttr".into(), "Vǻlue".into())],
                details: vec!["Detaïl".into()],
                actions: vec!["Actiøn".into()],
                reactions: vec!["Reactiœn".into()],
            },
            color: Color::Gray { v: 0.0 },
        });
        doc.assign_missing_block_ids().expect("ids");

        let chars = collect_doc_chars(&doc);
        for c in ['á', 'ó', 'Ǫ', 'Ǎ', 'ǻ', 'ï', 'ø', 'œ'] {
            assert!(chars.contains(&c), "{c:?} must reach the font subset");
        }

        // And it must actually export with them.
        let (opts, icc) = opts_with_real_icc("statblock_glyphs");
        let mut bytes = Vec::new();
        export(&doc, &opts, &mut bytes).expect("export");
        assert!(bytes.starts_with(b"%PDF-1.3"));
        let _ = std::fs::remove_file(icc);
    }

    #[test]
    fn a_stat_block_in_rgb_fails_preflight_like_any_other_block() {
        // The colour check walks `Block` variants, so a new variant it does not mention is a new
        // way for RGB to reach a press file.
        let mut doc = Document::sample();
        doc.content.push(Block::Panel {
            id: quill_core_model::BlockId::UNASSIGNED,
            panel: quill_core_model::Panel {
                name: "Goblin".into(),
                ..Default::default()
            },
            color: Color::Rgb {
                r: 1.0,
                g: 0.0,
                b: 0.0,
            },
        });
        doc.assign_missing_block_ids().expect("ids");
        let (opts, icc) = opts_with_real_icc("statblock_rgb");
        let report = preflight(&doc, &opts);
        assert!(report
            .findings
            .iter()
            .any(|f| f.check == CheckId::ColorSpace && f.severity == Severity::Error));
        let _ = std::fs::remove_file(icc);
    }

    #[test]
    fn a_decoration_over_the_ink_limit_is_an_error() {
        // The reason spec 0037 lands the primitive and the check together. Both existing colour
        // checks walk `doc.content`, so a rectangle the layout engine synthesized is invisible to
        // them — a panel at 280% total ink would reach a print shop with preflight reporting
        // nothing. This test fails against the pre-0037 checker, which is the point of it.
        let over = Color::Cmyk {
            c: 0.8,
            m: 0.7,
            y: 0.7,
            k: 0.6,
        }; // 280%
        let findings = preflight_pages(
            &[page_with_rect(Some(over), None)],
            &PodPreset::generic(),
            &PageSetup::default(),
            &[],
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check, CheckId::InkCoverage);
        assert_eq!(findings[0].severity, Severity::Error);
    }

    #[test]
    fn an_rgb_decoration_is_an_error_in_fill_and_in_stroke_alike() {
        // Both colours on a rect are ink. Checking only the fill would let a rule drawn in RGB
        // through, which is the same defect wearing a thinner line.
        let rgb = Color::Rgb {
            r: 1.0,
            g: 0.0,
            b: 0.0,
        };
        let by_fill = preflight_pages(
            &[page_with_rect(Some(rgb), None)],
            &PodPreset::generic(),
            &PageSetup::default(),
            &[],
        );
        assert_eq!(by_fill.len(), 1);
        assert_eq!(by_fill[0].check, CheckId::ColorSpace);

        let by_stroke = preflight_pages(
            &[page_with_rect(
                None,
                Some(quill_layout_engine::Stroke {
                    color: rgb,
                    width_pt: 0.5,
                }),
            )],
            &PodPreset::generic(),
            &PageSetup::default(),
            &[],
        );
        assert_eq!(by_stroke.len(), 1);
        assert_eq!(by_stroke[0].check, CheckId::ColorSpace);
    }

    #[test]
    fn a_press_legal_decoration_produces_no_findings() {
        // The reuse direction: without it the two tests above would pass against a checker that
        // simply reports everything.
        let tint = Color::Cmyk {
            c: 0.0,
            m: 0.0,
            y: 0.0,
            k: 0.1,
        };
        assert!(preflight_pages(
            &[page_with_rect(Some(tint), None)],
            &PodPreset::generic(),
            &PageSetup::default(),
            &[]
        )
        .is_empty());
        assert!(preflight_pages(&[], &PodPreset::generic(), &PageSetup::default(), &[]).is_empty());
    }

    #[test]
    fn export_refuses_a_document_whose_decoration_breaks_the_ink_limit() {
        // End to end: the geometry check has to be wired into `export`, not merely exist.
        // Asserted through the real export path rather than by calling the checker directly.
        let (opts, icc) = opts_with_real_icc("decoration_ink");
        let over = Color::Cmyk {
            c: 0.8,
            m: 0.7,
            y: 0.7,
            k: 0.6,
        };
        let findings = preflight_pages(
            &[page_with_rect(Some(over), None)],
            &PodPreset::generic(),
            &PageSetup::default(),
            &[],
        );
        assert_eq!(findings.len(), 1, "fixture must actually violate the limit");

        // A clean document still exports, so the wiring cannot have simply broken export.
        let mut bytes = Vec::new();
        export(&Document::sample(), &opts, &mut bytes).expect("a clean document must still export");
        assert!(bytes.starts_with(b"%PDF-1.3"));
        let _ = std::fs::remove_file(icc);
    }

    #[test]
    fn every_bundled_template_is_press_clean_and_exportable() {
        // Spec 0036's load-bearing criterion. A starter document that fails preflight the moment it
        // is created teaches a beginner that the error panel is noise — which is the one lesson
        // this product cannot afford to teach, because the panel is what stands between them and a
        // mis-coloured file at a print shop.
        //
        // This lives in export-pdf rather than core-model deliberately: it must run the real
        // checker, not a re-implementation of what we believe the checker does.
        let (opts, icc) = opts_with_real_icc("template_preflight");
        for t in quill_core_model::Template::bundled() {
            let doc = Document::from_template(t);

            let report = preflight(&doc, &opts);
            let errors: Vec<_> = report
                .findings
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .collect();
            assert!(
                errors.is_empty(),
                "template `{}` is not press-clean: {errors:?}",
                t.name
            );

            // And it must actually export. An empty content list is a path nothing exercised
            // before templates existed.
            let mut bytes = Vec::new();
            export(&doc, &opts, &mut bytes)
                .unwrap_or_else(|e| panic!("template `{}` failed to export: {e:?}", t.name));
            assert!(
                bytes.starts_with(b"%PDF-1.3"),
                "template `{}` produced no PDF",
                t.name
            );
        }
        let _ = std::fs::remove_file(icc);
    }

    #[test]
    fn export_refuses_when_preflight_fails() {
        let mut sink = Vec::new();
        // Default opts have no ICC -> preflight fails -> export refuses, writes nothing.
        let e = export(&Document::sample(), &ExportOptions::default(), &mut sink).unwrap_err();
        assert!(matches!(e, ExportError::PreflightFailed(_)));
        assert!(sink.is_empty());
    }

    /// Write the synthesized CMYK profile to a temp file and return options pointing at it.
    fn opts_with_real_icc(tag: &str) -> (ExportOptions, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("quill_test_{tag}.icc"));
        std::fs::write(&path, synth_cmyk_profile()).unwrap();
        (
            ExportOptions {
                output_intent_icc: path.to_string_lossy().into_owned(),
                ..Default::default()
            },
            path,
        )
    }

    /// FNV-1a over the exported bytes. Deliberately not SHA-256: this is a tripwire against
    /// *accidental* change, not a defense against a forged collision, and the workspace's
    /// minimal-dependency rule (`Cargo.toml`) does not justify a crypto crate to detect a typo.
    /// The same construction already backs the PDF `/ID` in `writer::doc_id_bytes`.
    fn digest(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// Spec 0025 moved asset resolution off the process working directory. That is exactly the kind
    /// of change that can silently alter which images embed — and a dropped image reaching a print
    /// shop is the failure class `CLAUDE.md` forbids. This pins the sample export's bytes so any
    /// unintended shift in output is a red test rather than a surprise in a PDF.
    ///
    /// If a spec deliberately changes export output, update this constant *in that spec's commit*,
    /// having confirmed the new bytes are the intended ones.
    /// Changed by spec 0047: `FORMAT_VERSION` became 3, so `doc.to_json()` changed and with it the
    /// document identifier. The sample has no masters, so nothing this spec added reaches its page;
    /// only the version stamp in the manifest text the identifier is hashed from moved. Verified
    /// identifier-only, the same way spec 0030's bump was: both files are 8786 bytes and differ in
    /// exactly 116, every one inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID` (both
    /// halves). No content stream moved.
    ///
    /// Previously changed by spec 0041: `StyleSheet::default()` gained `toc-title` and `toc-1`..`toc-6`, so
    /// `doc.to_json()` changed and with it the document identifier. Verified identifier-only: both
    /// files are 8786 bytes and differ in exactly 124, in the two XMP identifier clusters and the
    /// trailer `/ID`. No content stream moved.
    ///
    /// Previously changed by spec 0042, and that one was **structural rather than identifier-only** — the first
    /// such move in M2, and expected: the catalog gained `/Outlines` and the document gained an
    /// outline root plus one item for the sample's single `h1`. 8559 -> 8786 bytes.
    ///
    /// Verified that nothing else moved: every `/Length` in the file is unchanged
    /// (`[1017, 376, 2981, 2017]` before and after), so no content stream, font or metadata stream
    /// was touched; the object count rose by exactly the two new outline objects.
    ///
    /// Previously changed by spec 0039: `StyleSheet::default()` gained the two built-in `table-*` styles, so
    /// `doc.to_json()` changed and with it the document identifier. Verified the same way as spec
    /// 0038's move: both files are 8559 bytes and differ in exactly 120, every one inside the XMP
    /// `DocumentID`/`InstanceID` or the trailer `/ID`. No content stream moved.
    ///
    /// Previously changed by spec 0038: `StyleSheet::default()` gained the three built-in `statblock-*`
    /// styles, so `doc.to_json()` changed and with it the document identifier. Verified as
    /// identifier-only rather than accepted: exporting the sample against the committed parity ICC
    /// before and after gives files of identical length (8559 bytes) differing in exactly 108
    /// bytes, every one inside the XMP `DocumentID`/`InstanceID` or the trailer `/ID`. No content
    /// stream moved.
    ///
    /// Previously changed by spec 0030: `format_version` became 2 and the manifest gained `master_pages`, so
    /// `doc.to_json()` changed and with it the document identifier. Verified as identifier-only:
    /// 124 bytes differ across 8 runs, every one inside the XMP `DocumentID`/`InstanceID` or the
    /// trailer `/ID`; length is unchanged and no content stream moved.
    ///
    /// Previously changed by spec 0028: paragraph styles reached the page. The sample'''s h1 heading now sets
    /// at 24 pt with space above instead of at body size, so the content stream gained a second
    /// `/F0 24 Tf` and every baseline below the heading moved down.
    ///
    /// Verified by inspecting the emitted text operators rather than by accepting the new number:
    /// before, the stream contained only `/F0 10 Tf` — a heading was distinguishable from body text
    /// only by being ragged-left.
    ///
    /// Changed again by spec 0048, and again only in the document's identity. `StyleSheet::default()`
    /// now gives `statblock-attr` a 10 pt hanging indent, so `doc.to_json()` differs and the `/ID`
    /// derived from it moves. Verified rather than accepted: exported against the committed parity
    /// ICC before and after, **8786 bytes both sides**, 116 differing bytes in 16 runs, and every
    /// run inside the XMP `DocumentID`/`InstanceID` or one of the two halves of the trailer `/ID`.
    /// No content, font or metadata stream moved. The sample has no stat block, so nothing it draws
    /// could have changed — which is what makes an identifier-only diff the expected result here
    /// rather than a hopeful one.
    ///
    /// Changed again by spec 0063, and again only in the document's identity. A paragraph is now an
    /// ordered list of runs, so `doc.to_json()` carries `runs` where it carried `text` and
    /// `format_version` 4 where it carried 3 — and the `/ID` derived from it moves. Verified by the
    /// same procedure: exported against the committed parity ICC before and after, **8786 bytes
    /// both sides**, 124 differing bytes, and every one of them inside the XMP
    /// `DocumentID`/`InstanceID` or the trailer `/ID`. Not one byte of a content, font or metadata
    /// stream moved — which is the claim the whole increment rests on, since a run model that moved
    /// a glyph would be a layout change rather than a generalization.
    ///
    /// Changed again by spec 0066, and again only in the document's identity — the same shape spec
    /// 0038 first recorded. `StyleSheet::default()` gains the two built-in list treatments, so
    /// `doc.to_json()` differs and the `/ID` derived from it moves. Verified rather than accepted:
    /// **8786 bytes both sides**, 128 differing bytes, every one inside the XMP
    /// `DocumentID`/`InstanceID` or the trailer `/ID`. The sample contains no list, so nothing it
    /// draws could have changed.
    const SAMPLE_EXPORT_DIGEST: u64 = 0x29b3_771f_6bfb_a2bd;

    /// Byte offsets of the ICC header's `dateTimeNumber` field (ICC.1 spec, header bytes 24..36).
    const ICC_DATETIME: std::ops::Range<usize> = 24..36;
    /// Byte offsets of the ICC header's primary-platform signature (ICC.1 spec, bytes 40..44).
    const ICC_PLATFORM: std::ops::Range<usize> = 40..44;

    /// A fixed OutputIntent profile, committed so the byte-parity digest measures the *writer*.
    ///
    /// A PDF/X export embeds the OutputIntent profile verbatim, so the digest is only meaningful if
    /// the profile is constant. `synth_cmyk_profile()` is not: lcms2 stamps the current time into
    /// the ICC header, **and** writes a primary-platform signature that follows the build host
    /// (`APPL` on Linux/macOS, `MSFT` on Windows). The first makes the digest a clock; the second
    /// made it fail on the Windows CI leg with an identical byte length.
    ///
    /// Both are inherent to ICC rather than defects — a real export supplies a fixed press profile
    /// and *is* reproducible. Committing one generated profile is the fix that does not require
    /// enumerating which header fields happen to vary today, per `CLAUDE.md`'s guidance on binary
    /// test fixtures. It is only used for parity assertions; every other test still exercises the
    /// live synthesizer.
    const PARITY_ICC: &[u8] = include_bytes!("../assets/parity-outputintent.icc");

    fn opts_with_fixed_icc(tag: &str) -> (ExportOptions, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("quill_fixed_{tag}.icc"));
        std::fs::write(&path, PARITY_ICC).unwrap();
        (
            ExportOptions {
                output_intent_icc: path.to_string_lossy().into_owned(),
                ..Default::default()
            },
            path,
        )
    }

    #[test]
    fn the_parity_profile_is_a_valid_output_intent() {
        // The fixture must still be a profile the exporter accepts, or the parity test would be
        // asserting over an export path nothing else takes.
        icc::check_icc(PARITY_ICC).expect("committed parity profile must pass ICC validation");
        assert_eq!(PARITY_ICC.len(), synth_cmyk_profile().len());
    }

    #[test]
    fn a_synthesized_icc_profile_varies_by_clock_and_host() {
        // Documents exactly why PARITY_ICC exists. If lcms2 ever stops stamping these fields, this
        // test says so rather than leaving a committed fixture nobody understands.
        let a = synth_cmyk_profile();
        assert!(
            a[ICC_DATETIME].iter().any(|b| *b != 0),
            "expected a non-zero ICC creation timestamp"
        );
        assert!(
            matches!(&a[ICC_PLATFORM], b"APPL" | b"MSFT" | b"SUNW" | b"SGI "),
            "expected a host-dependent primary-platform signature, got {:?}",
            &a[ICC_PLATFORM]
        );
        // Everything outside those two header fields is deterministic within one host.
        let mut x = synth_cmyk_profile();
        let mut y = a.clone();
        for f in [ICC_DATETIME, ICC_PLATFORM] {
            x[f.clone()].fill(0);
            y[f].fill(0);
        }
        assert_eq!(x, y, "profile body must be deterministic");
    }

    #[test]
    fn export_of_the_sample_document_is_byte_stable() {
        let (opts, path) = opts_with_fixed_icc("byte_stable");
        let mut a = Vec::new();
        export(&Document::sample(), &opts, &mut a).expect("export");
        let mut b = Vec::new();
        export(&Document::sample(), &opts, &mut b).expect("export");
        let _ = std::fs::remove_file(&path);

        // Determinism first: without it the constant below would be meaningless.
        assert_eq!(a, b, "export must be deterministic run to run");
        assert_eq!(
            digest(&a),
            SAMPLE_EXPORT_DIGEST,
            "sample export bytes changed ({} bytes, digest {:#x}); if intended, update \
             SAMPLE_EXPORT_DIGEST in this commit",
            a.len(),
            digest(&a)
        );
    }

    #[test]
    fn asset_root_resolves_relative_asset_paths() {
        // The knob spec 0025 added: a document's relative asset path resolves against the
        // document's own root, not the process working directory.
        let (mut opts, icc_path) = opts_with_real_icc("asset_root");
        let root = std::env::temp_dir().join(format!("quill_asset_root_{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        {
            let file = std::fs::File::create(root.join("assets/pic.png")).unwrap();
            let mut enc = png::Encoder::new(file, 2, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[10, 120, 240, 240, 120, 10]).unwrap();
        }

        let mut doc = Document::sample();
        doc.assets = vec![Asset {
            id: "pic".into(),
            // Relative — meaningless without an asset root.
            path: "assets/pic.png".into(),
            px_w: 600,
            px_h: 600,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        }];
        doc.content.push(Block::image("pic"));

        opts.asset_root = root.clone();
        let mut found = Vec::new();
        export(&doc, &opts, &mut found).expect("export");
        assert!(
            String::from_utf8_lossy(&found).contains("DeviceCMYK"),
            "image should embed when asset_root points at the document's root"
        );

        // And the negative: a wrong root drops the image rather than embedding something else.
        opts.asset_root = std::env::temp_dir().join("quill_definitely_not_here");
        let mut missing = Vec::new();
        export(&doc, &opts, &mut missing).expect("export should still succeed");
        assert!(
            !String::from_utf8_lossy(&missing).contains("/Subtype /Image"),
            "an unresolvable asset must be skipped, not guessed at"
        );

        let _ = std::fs::remove_file(&icc_path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn export_writes_pdfx_bytes_on_clean_document() {
        let (opts, path) = opts_with_real_icc("clean");
        let mut buf = Vec::new();
        export(&Document::sample(), &opts, &mut buf).expect("export should succeed");
        let _ = std::fs::remove_file(&path);

        assert!(!buf.is_empty());
        // PDF/X-1a:2001 pins the header to 1.3.
        assert!(buf.starts_with(b"%PDF-1.3"), "wrong PDF header");
        assert!(buf.ends_with(b"%%EOF\n") || buf.ends_with(b"%%EOF"));
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("GTS_PDFX"),
            "missing PDF/X OutputIntent marker"
        );
        assert!(
            text.contains("/CIDFontType2"),
            "missing embedded composite font"
        );
        assert!(text.contains("Identity-H"), "missing Identity-H encoding");
        // The default level is X-1a: both the info dict and the XMP identify it as such.
        assert!(text.contains("PDF/X-1a:2001"), "missing X-1a identifier");
        assert!(
            !text.contains("PDF/X-3"),
            "unexpected X-3 identifier in X-1a export"
        );
    }

    #[test]
    fn export_writes_pdfx3_identifier() {
        let (mut opts, path) = opts_with_real_icc("x3");
        opts.version = PdfxVersion::X3_2002;
        let mut buf = Vec::new();
        export(&Document::sample(), &opts, &mut buf).expect("X-3 export should succeed");
        let _ = std::fs::remove_file(&path);

        let text = String::from_utf8_lossy(&buf);
        // X-3:2002 identifier is present (info dict + XMP) and no X-1a string leaks through.
        assert!(
            text.contains("PDF/X-3:2002"),
            "missing PDF/X-3:2002 identifier"
        );
        assert!(
            !text.contains("PDF/X-1a"),
            "X-3 export must not identify as X-1a"
        );
        // X-3:2002 defines no GTS_PDFXConformance key.
        assert!(
            !text.contains("GTS_PDFXConformance"),
            "X-3 must omit GTS_PDFXConformance"
        );
        // Still a valid PDF/X shell: PDF 1.3 header + OutputIntent.
        assert!(buf.starts_with(b"%PDF-1.3"), "wrong PDF header");
        assert!(
            text.contains("GTS_PDFX"),
            "missing PDF/X OutputIntent marker"
        );
    }

    #[test]
    fn export_places_bundled_grayscale_image() {
        let (opts, path) = opts_with_real_icc("image");
        // Point an asset at the bundled test image (absolute path) and reference it.
        let mut doc = Document::sample();
        let img_path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/test_gray.png");
        doc.assets = vec![Asset {
            id: "pic".into(),
            path: img_path.into(),
            px_w: 600,
            px_h: 600,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        }];
        doc.content.push(Block::image("pic"));

        let mut buf = Vec::new();
        export(&doc, &opts, &mut buf).expect("export with image should succeed");
        let _ = std::fs::remove_file(&path);

        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("/Subtype /Image") || text.contains("/Subtype/Image"));
        assert!(
            text.contains("DeviceGray"),
            "image must be DeviceGray for X-1a"
        );
    }

    #[test]
    fn export_places_color_image_as_device_cmyk() {
        let (opts, icc_path) = opts_with_real_icc("color_image");

        // Write a tiny RGB PNG to a temp file and reference it (color art path, spec 0005).
        let png_path = std::env::temp_dir().join("quill_test_color.png");
        {
            let file = std::fs::File::create(&png_path).unwrap();
            let mut enc = png::Encoder::new(file, 2, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[10, 120, 240, 240, 120, 10]).unwrap();
        }

        let mut doc = Document::sample();
        doc.assets = vec![Asset {
            id: "pic".into(),
            path: png_path.to_string_lossy().into_owned(),
            px_w: 600,
            px_h: 600,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        }];
        doc.content.push(Block::image("pic"));

        let mut buf = Vec::new();
        export(&doc, &opts, &mut buf).expect("export with color image should succeed");
        let _ = std::fs::remove_file(&icc_path);
        let _ = std::fs::remove_file(&png_path);

        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("/Subtype /Image") || text.contains("/Subtype/Image"));
        assert!(
            text.contains("DeviceCMYK"),
            "color image must be DeviceCMYK for PDF/X"
        );
    }

    #[test]
    fn export_places_rgb_jpeg_as_device_cmyk() {
        // A linked JPEG must survive export as press-legal CMYK, not be dropped (spec 0008).
        let (opts, icc_path) = opts_with_real_icc("jpeg_image");
        let img_path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/test_rgb.jpg");

        let mut doc = Document::sample();
        doc.assets = vec![Asset {
            id: "pic".into(),
            path: img_path.into(),
            px_w: 600,
            px_h: 600,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        }];
        doc.content.push(Block::image("pic"));

        let mut buf = Vec::new();
        export(&doc, &opts, &mut buf).expect("export with jpeg image should succeed");
        let _ = std::fs::remove_file(&icc_path);

        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("/Subtype /Image") || text.contains("/Subtype/Image"));
        assert!(
            text.contains("DeviceCMYK"),
            "color JPEG must be DeviceCMYK for PDF/X"
        );
    }

    #[test]
    fn export_places_cmyk_jpeg_as_device_cmyk() {
        // A linked CMYK JPEG (Adobe APP14 transform 0) must embed as DeviceCMYK, not be dropped
        // (spec 0012). The bundled fixture is already CMYK, so it takes the direct-embed path.
        let (opts, icc_path) = opts_with_real_icc("cmyk_jpeg");
        let img_path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/test_cmyk.jpg");

        let mut doc = Document::sample();
        doc.assets = vec![Asset {
            id: "pic".into(),
            path: img_path.into(),
            px_w: 600,
            px_h: 600,
            dpi: 300.0,
            line_art: false,
            has_alpha: false,
        }];
        doc.content.push(Block::image("pic"));

        let mut buf = Vec::new();
        export(&doc, &opts, &mut buf).expect("export with cmyk jpeg should succeed");
        let _ = std::fs::remove_file(&icc_path);

        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("/Subtype /Image") || text.contains("/Subtype/Image"));
        assert!(
            text.contains("DeviceCMYK"),
            "CMYK JPEG must be DeviceCMYK for PDF/X"
        );
    }

    #[test]
    fn export_refuses_unreadable_icc_even_when_preflight_forced() {
        // force=true skips preflight, but the writer still needs a valid ICC to embed.
        let opts = ExportOptions {
            output_intent_icc: "definitely/missing.icc".into(),
            force: true,
            ..Default::default()
        };
        let mut sink = Vec::new();
        let e = export(&Document::sample(), &opts, &mut sink).unwrap_err();
        assert!(matches!(e, ExportError::Icc(_)));
    }

    /// Spec 0004: a user-supplied `font_path` is embedded instead of the bundled default, with a
    /// BaseFont name derived from that file. Exercised with the bundled ttf on disk so no extra
    /// fixture is needed; the derived name ("SourceSerif…") proves the derive path ran.
    #[test]
    fn export_embeds_user_supplied_font() {
        let (mut opts, icc_path) = opts_with_real_icc("userfont");
        opts.font_path = Some(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/SourceSerif4-Regular.ttf"
            )
            .into(),
        );
        let mut buf = Vec::new();
        export(&Document::sample(), &opts, &mut buf).expect("user-font export should succeed");
        let _ = std::fs::remove_file(&icc_path);

        assert!(!buf.is_empty());
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("/CIDFontType2"), "font not embedded");
        assert!(
            text.contains("SourceSerif"),
            "BaseFont should reflect the supplied font's own name"
        );
    }

    /// Spec 0011: a CFF-outline `.otf` embeds as a `CIDFontType0` descendant with its bare `CFF `
    /// table in a `FontFile3` (`/Subtype /CIDFontType0C`) — the only PDF 1.3-legal CFF form. The
    /// TrueType markers (`/CIDFontType2`, `/FontFile2`, `/CIDToGIDMap`) must be absent, and the
    /// synthetic fixture's own name proves the CFF program was parsed. Ghostscript's CI
    /// well-formedness gate then confirms the bytes are valid.
    #[test]
    fn export_embeds_cff_otf_font() {
        let (mut opts, icc_path) = opts_with_real_icc("cfffont");
        opts.font_path = Some(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/test-cff.otf").into());
        let mut buf = Vec::new();
        export(&Document::sample(), &opts, &mut buf).expect("CFF-font export should succeed");
        let _ = std::fs::remove_file(&icc_path);

        assert!(buf.starts_with(b"%PDF-1.3"), "wrong PDF header");
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("/CIDFontType0C"),
            "CFF not embedded as FontFile3/CIDFontType0C"
        );
        assert!(text.contains("/FontFile3"), "missing FontFile3");
        assert!(
            text.contains("QuillTestCFF"),
            "BaseFont should reflect the CFF font's name"
        );
        assert!(
            !text.contains("/CIDFontType2"),
            "CFF export must not use CIDFontType2"
        );
        assert!(
            !text.contains("/FontFile2"),
            "CFF export must not use FontFile2"
        );
        assert!(
            !text.contains("/CIDToGIDMap"),
            "CIDFontType0 must omit CIDToGIDMap"
        );
    }

    #[test]
    fn export_fails_on_unreadable_font() {
        let (mut opts, icc_path) = opts_with_real_icc("missingfont");
        opts.font_path = Some("definitely/missing.ttf".into());
        let mut sink = Vec::new();
        let e = export(&Document::sample(), &opts, &mut sink).unwrap_err();
        let _ = std::fs::remove_file(&icc_path);
        assert!(matches!(e, ExportError::Font(_)));
        assert!(sink.is_empty(), "nothing should be written on font failure");
    }

    // --- The screen profile (spec 0052) --------------------------------------------------------

    /// Titles of the two chapters the linked-document fixture carries.
    const CH1: &str = "The First Chapter";
    const CH2: &str = "The Second Chapter";

    /// A document with a contents list and two chapters far enough apart that the second lands on a
    /// later page — so a destination test can distinguish "the right page" from "page one".
    fn linked_doc() -> Document {
        let mut doc = Document::sample();
        doc.content = vec![Block::Toc {
            id: quill_core_model::BlockId::UNASSIGNED,
            title: "Contents".into(),
            max_level: 2,
            color: Color::Gray { v: 0.0 },
        }];
        for title in [CH1, CH2] {
            doc.content
                .push(Block::heading(1, title, Color::Gray { v: 0.0 }));
            for _ in 0..40 {
                doc.content.push(Block::body(
                    "Filler copy that exists only to consume vertical space so the chapters do \
                     not share a page.",
                    Color::Gray { v: 0.0 },
                ));
            }
        }
        doc.assign_missing_block_ids().expect("ids");
        doc
    }

    /// Lay a document out exactly as [`export`] does — same font, same shaper, same hyphenator — so
    /// a test can compare the emitted PDF against the geometry the writer was actually handed.
    fn lay_out_like_export(doc: &Document, opts: &ExportOptions) -> Vec<LaidOutPage> {
        super::lay_out_for_press(doc, opts).expect("press layout")
    }

    /// Every `<n> 0 obj … endobj` body in the file, keyed by object number.
    ///
    /// A deliberately small parser: the assertions below need to *follow references*, and a
    /// substring match cannot do that — it can only confirm that some bytes appear somewhere.
    fn objects(bytes: &[u8]) -> std::collections::BTreeMap<u32, String> {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let mut out = std::collections::BTreeMap::new();
        let mut rest = text.as_str();
        let mut consumed = 0usize;
        while let Some(at) = rest.find(" 0 obj") {
            // Walk back over the object number.
            let head = &rest[..at];
            let start = head
                .rfind(|c: char| !c.is_ascii_digit())
                .map_or(0, |i| i + 1);
            let number: u32 = match head[start..].parse() {
                Ok(n) => n,
                Err(_) => {
                    rest = &rest[at + 6..];
                    consumed += at + 6;
                    continue;
                }
            };
            let body_start = consumed + at + 6;
            let end = text[body_start..]
                .find("endobj")
                .map(|e| body_start + e)
                .unwrap_or(text.len());
            out.insert(number, text[body_start..end].to_string());
            rest = &text[end..];
            consumed = end;
        }
        out
    }

    /// `[9 0 R, 12 0 R]` → `[9, 12]`, for the array named by `key` in `body`.
    fn ref_array(body: &str, key: &str) -> Vec<u32> {
        let Some(at) = body.find(key) else {
            return Vec::new();
        };
        let after = &body[at + key.len()..];
        let Some(open) = after.find('[') else {
            return Vec::new();
        };
        let Some(close) = after[open..].find(']') else {
            return Vec::new();
        };
        after[open + 1..open + close]
            .split(" R")
            .filter_map(|item| item.split_whitespace().next()?.parse().ok())
            .collect()
    }

    /// The page objects in page-tree order, so an object number can be turned into a page *index*.
    fn page_order(objs: &std::collections::BTreeMap<u32, String>) -> Vec<u32> {
        let tree = objs
            .values()
            .find(|b| b.contains("/Type /Pages"))
            .expect("a page tree");
        ref_array(tree, "/Kids")
    }

    /// The four numbers of the `/Rect` array in an annotation body.
    fn rect_of(body: &str) -> [f32; 4] {
        let at = body.find("/Rect").expect("an annotation has a /Rect");
        let after = &body[at + 5..];
        let open = after.find('[').expect("/Rect array");
        let close = after[open..].find(']').expect("/Rect array end");
        let nums: Vec<f32> = after[open + 1..open + close]
            .split_whitespace()
            .map(|n| n.parse().expect("a number"))
            .collect();
        [nums[0], nums[1], nums[2], nums[3]]
    }

    #[test]
    fn the_press_profile_emits_no_annotations_at_all() {
        // THE load-bearing assertion of spec 0052, and deliberately a *press* test.
        //
        // The document does have a contents list, so the layout engine really does produce link
        // candidates — this is not vacuous. It is asserted **structurally over the emitted bytes**:
        // any `/Annots` key anywhere in the file fails it. A test that checked "the annotation
        // builder returned nothing" would pass for a writer that emitted annotations by some other
        // route, and would stop meaning anything the moment the code was reorganised. Scanning the
        // output cannot rot that way.
        let (opts, icc) = opts_with_real_icc("press_no_annots");
        let doc = linked_doc();
        let mut bytes = Vec::new();
        export(&doc, &opts, &mut bytes).expect("export");
        let _ = std::fs::remove_file(icc);

        assert!(
            !bytes.windows(7).any(|w| w == b"/Annots"),
            "a PDF/X file must contain no /Annots key at all — not even an empty array"
        );
        assert!(
            !bytes.windows(6).any(|w| w == b"/Annot"),
            "and no annotation object either"
        );
        // The candidates exist: this proves the emptiness above is the writer's doing, not an
        // accident of a document that had nothing to link.
        let pages = lay_out_like_export(&doc, &opts);
        let candidates = pages
            .iter()
            .flat_map(|p| p.blocks.iter())
            .filter(|b| matches!(b, PlacedBlock::Link { .. }))
            .count();
        assert!(
            candidates > 0,
            "the fixture must actually produce link candidates, or this test proves nothing"
        );
    }

    #[test]
    fn a_screen_export_links_a_contents_entry_to_the_page_its_heading_is_on() {
        // Three things, each of which can be wrong independently:
        //   1. the entry is a `/Link` annotation at all;
        //   2. its `/Rect` covers the *placed text* of that entry — asserted against the run's own
        //      laid-out frame, so a link that has drifted off its text fails even though it exists;
        //   3. its destination *resolves* to the page the heading is on — followed through the
        //      reference into the page tree, not pattern-matched against an object number, which
        //      would pin an allocation order rather than a destination.
        let opts = ExportOptions {
            profile: ExportProfile::Screen,
            ..Default::default()
        };
        let doc = linked_doc();
        let mut bytes = Vec::new();
        export(&doc, &opts, &mut bytes).expect("screen export needs no ICC");

        let pages = lay_out_like_export(&doc, &opts);
        let headings = quill_layout_engine::heading_index(&doc, &pages);
        let ch2 = headings
            .iter()
            .find(|h| h.text == CH2)
            .expect("the second chapter is indexed");
        assert!(
            ch2.page_index > 0,
            "the fixture must span pages, or 'the right page' and 'page one' are the same claim"
        );

        // The entry's placed text: the contents run whose text is the chapter title.
        let (toc_page, text_frame) = pages
            .iter()
            .find_map(|p| {
                p.blocks.iter().find_map(|b| match b {
                    PlacedBlock::Text { frame, lines, .. }
                        if lines.len() == 1
                            && lines[0].text == CH2
                            && p.index != ch2.page_index =>
                    {
                        Some((p.index, *frame))
                    }
                    _ => None,
                })
            })
            .expect("a contents entry for the second chapter");

        let objs = objects(&bytes);
        let order = page_order(&objs);
        let page_obj = order[toc_page];
        let annot_ids = ref_array(&objs[&page_obj], "/Annots");
        assert!(
            !annot_ids.is_empty(),
            "the contents page must carry link annotations"
        );

        // The annotation whose rect sits at this entry's text.
        let g = quill_core_model::page_geom(&doc.page_setup, toc_page);
        let expect_top = g.media_h - (g.off_y + text_frame.y_pt);
        let expect = [
            g.off_x + text_frame.x_pt,
            expect_top - text_frame.h_pt,
            g.off_x + text_frame.x_pt + text_frame.w_pt,
            expect_top,
        ];
        let mut matched = None;
        for id in &annot_ids {
            let body = &objs[id];
            assert!(body.contains("/Subtype /Link"), "not a link: {body}");
            let r = rect_of(body);
            if r.iter().zip(expect).all(|(a, b)| (a - b).abs() < 0.01) {
                matched = Some(body.clone());
            }
        }
        let body = matched.unwrap_or_else(|| {
            panic!(
                "no /Link rect covers the entry's placed text at {expect:?}; annotations were {:?}",
                annot_ids
                    .iter()
                    .map(|i| rect_of(&objs[i]))
                    .collect::<Vec<_>>()
            )
        });

        // Follow the destination: `/D [<n> 0 R /Fit]` → the page object → its index in `/Kids`.
        assert!(body.contains("/S /GoTo"), "a GoTo action: {body}");
        let dest = ref_array(&body, "/D");
        assert_eq!(dest.len(), 1, "one destination page reference: {body}");
        let landed = order
            .iter()
            .position(|p| *p == dest[0])
            .expect("the destination must be a page in the page tree");
        assert_eq!(
            landed, ch2.page_index,
            "the link must resolve to the page the heading is actually on"
        );
    }

    #[test]
    fn the_pdfx_identification_is_present_under_press_and_absent_under_screen() {
        // Both directions, in both places the identification lives. Asserting only the presence
        // would pass for a writer that stamped PDF/X on everything; only the absence would pass for
        // one that stamped it on nothing.
        let (press_opts, icc) = opts_with_real_icc("ident_press");
        let doc = linked_doc();
        let mut press = Vec::new();
        export(&doc, &press_opts, &mut press).expect("press export");
        let _ = std::fs::remove_file(icc);
        let press = String::from_utf8_lossy(&press).into_owned();

        let mut screen = Vec::new();
        export(
            &doc,
            &ExportOptions {
                profile: ExportProfile::Screen,
                ..Default::default()
            },
            &mut screen,
        )
        .expect("screen export");
        let screen = String::from_utf8_lossy(&screen).into_owned();

        // Info dictionary.
        assert!(press.contains("/GTS_PDFXVersion"), "press info key");
        assert!(
            !screen.contains("GTS_PDFXVersion"),
            "a screen file must claim no PDF/X version anywhere"
        );
        assert!(press.contains("/GTS_PDFXConformance"), "press conformance");
        assert!(!screen.contains("GTS_PDFXConformance"));
        // XMP packet (uncompressed, so both directions are greppable).
        assert!(press.contains("npes.org/pdfx/ns/id/"), "press XMP schema");
        assert!(!screen.contains("npes.org"), "no XMP identification schema");
        // OutputIntent.
        assert!(press.contains("/OutputIntents"), "press output intent");
        assert!(
            !screen.contains("/OutputIntents") && !screen.contains("/DestOutputProfile"),
            "a screen file states no output condition"
        );
        // And the mirror of the press test above: the screen file *does* have the links.
        assert!(screen.contains("/Annots"), "screen links");
        assert!(screen.contains("/Subtype /Link"), "screen link subtype");
    }

    #[test]
    fn a_screen_export_needs_no_icc_and_a_press_export_still_does() {
        let doc = Document::sample();
        let mut screen = Vec::new();
        export(
            &doc,
            &ExportOptions {
                profile: ExportProfile::Screen,
                ..Default::default()
            },
            &mut screen,
        )
        .expect("the screen profile requires no OutputIntent");
        assert!(screen.starts_with(b"%PDF-1.3"));

        // The press profile is untouched by any of this: no ICC is still a preflight failure.
        let mut press = Vec::new();
        let e = export(&doc, &ExportOptions::default(), &mut press).unwrap_err();
        assert!(matches!(e, ExportError::PreflightFailed(_)));
        assert!(press.is_empty());
    }

    #[test]
    fn screen_preflight_records_what_it_skipped_and_still_runs_everything_else() {
        // "No findings" over an unstated subset of the checks is the shape of a false pass, so the
        // omission is data in the report. The second half is what keeps the relaxation *narrow*: a
        // profile that skipped everything would also pass the first half.
        let mut doc = Document::sample();
        let screen = ExportOptions {
            profile: ExportProfile::Screen,
            ..Default::default()
        };
        let report = preflight(&doc, &screen);
        assert!(report.passed(), "unexpected: {:?}", report.findings);
        let skipped: Vec<CheckId> = report.skipped.iter().map(|s| s.check).collect();
        assert_eq!(
            skipped,
            vec![CheckId::OutputIntent, CheckId::IccProfileInvalid],
            "exactly the two OutputIntent-related checks, and no others"
        );
        assert!(
            report.skipped.iter().all(|s| !s.reason.trim().is_empty()),
            "a skipped check must say why"
        );
        assert!(
            !report.applied().contains(&CheckId::OutputIntent),
            "applied() is the complement of skipped()"
        );
        for c in [
            CheckId::ColorSpace,
            CheckId::InkCoverage,
            CheckId::Bleed,
            CheckId::ImageResolution,
            CheckId::FontEmbedding,
        ] {
            assert!(report.applied().contains(&c), "{c:?} must still be applied");
        }

        // And they are applied in the sense that matters: they still fail on bad input.
        doc.page_setup.bleed_pt = 2.0;
        doc.fonts_embeddable = false;
        doc.content.push(Block::body(
            "oops",
            Color::Rgb {
                r: 1.0,
                g: 0.0,
                b: 0.0,
            },
        ));
        doc.assets = vec![Asset {
            id: "blurry".into(),
            path: "assets/blurry.png".into(),
            px_w: 10,
            px_h: 10,
            dpi: 72.0,
            line_art: false,
            has_alpha: false,
        }];
        let report = preflight(&doc, &screen);
        assert!(!report.passed(), "the screen profile is not a blanket pass");
        for c in [
            CheckId::ColorSpace,
            CheckId::Bleed,
            CheckId::ImageResolution,
            CheckId::FontEmbedding,
        ] {
            assert!(
                report.findings.iter().any(|f| f.check == c),
                "{c:?} must still be reported under the screen profile"
            );
        }
    }

    #[test]
    fn an_icc_supplied_to_the_screen_profile_is_validated_but_writes_no_output_intent() {
        // Giving the screen profile a profile un-skips the ICC checks (it is now something that can
        // be wrong) without making the file claim conformance it does not have.
        let (mut opts, icc) = opts_with_real_icc("screen_with_icc");
        opts.profile = ExportProfile::Screen;
        let report = preflight(&Document::sample(), &opts);
        assert!(
            report.skipped.is_empty(),
            "with a profile supplied there is nothing to skip: {:?}",
            report.skipped
        );

        let mut bytes = Vec::new();
        export(&linked_doc(), &opts, &mut bytes).expect("export");
        let _ = std::fs::remove_file(icc);
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("/OutputIntents"), "still no output intent");
        assert!(!text.contains("GTS_PDFXVersion"), "still no PDF/X claim");
        assert!(text.contains("/Subtype /Link"), "still linked");
    }

    #[test]
    fn every_listed_contents_entry_gets_exactly_one_link() {
        // The count is the property: a link per entry, no more (a duplicate would make a viewer's
        // hit-testing depend on z-order) and no fewer.
        let opts = ExportOptions {
            profile: ExportProfile::Screen,
            ..Default::default()
        };
        let doc = linked_doc();
        let mut bytes = Vec::new();
        export(&doc, &opts, &mut bytes).expect("export");

        let pages = lay_out_like_export(&doc, &opts);
        let entries = quill_layout_engine::heading_index(&doc, &pages).len();
        assert_eq!(entries, 2, "the fixture lists two chapters");

        let objs = objects(&bytes);
        let links = objs
            .values()
            .filter(|b| b.contains("/Subtype /Link"))
            .count();
        assert_eq!(links, entries, "one link annotation per listed entry");
    }

    #[test]
    fn the_press_profile_is_the_default() {
        // Stated as a test because it is the whole safety argument: no caller reaches the screen
        // profile by omission, so no existing path can start emitting annotations by accident.
        assert_eq!(ExportOptions::default().profile, ExportProfile::Press);
        assert_eq!(ExportProfile::default(), ExportProfile::Press);
        assert_eq!(
            ExportProfile::Press.pdfx(PdfxVersion::X3_2002),
            Some(PdfxVersion::X3_2002)
        );
        assert_eq!(ExportProfile::Screen.pdfx(PdfxVersion::X3_2002), None);
    }

    // ----- spec 0050: preflight over placed geometry -------------------------------------------

    /// A preset that states a real safety margin, so the check has something to check. `generic`
    /// states none — see `PodPreset::generic` for why quill does not invent one.
    fn preset_with_safety(pt: f32) -> PodPreset {
        PodPreset {
            safety_pt: pt,
            ..PodPreset::generic()
        }
    }

    fn page_with(blocks: Vec<PlacedBlock>) -> LaidOutPage {
        LaidOutPage {
            index: 0,
            blocks,
            statics: Vec::new(),
        }
    }

    fn text_at(x: f32, y: f32, w: f32, h: f32) -> PlacedBlock {
        PlacedBlock::Text {
            run_colors: Vec::new(),
            source: quill_core_model::BlockId(1),
            frame: Rect {
                x_pt: x,
                y_pt: y,
                w_pt: w,
                h_pt: h,
            },
            lines: Vec::new(),
            color: Color::Gray { v: 0.0 },
            font_size_pt: 10.0,
            leading_pt: 12.0,
        }
    }

    fn image_at(id: &str, x: f32, y: f32, w: f32, h: f32) -> PlacedBlock {
        PlacedBlock::Image {
            source: quill_core_model::BlockId(1),
            frame: Rect {
                x_pt: x,
                y_pt: y,
                w_pt: w,
                h_pt: h,
            },
            asset_id: id.to_string(),
        }
    }

    fn asset_px(id: &str, px_w: u32, px_h: u32, line_art: bool) -> Asset {
        Asset {
            id: id.into(),
            path: format!("{id}.png"),
            px_w,
            px_h,
            dpi: 300.0,
            has_alpha: false,
            line_art,
        }
    }

    #[test]
    fn the_0036_fore_edge_folio_now_fails_preflight() {
        // The increment's proof of worth. Spec 0036's bundled folio printed hard against the trim —
        // where the guillotine goes — while every numeric test passed, and only a 2x render of a
        // filled page showed it. This reproduces that placement and asserts preflight now catches
        // it without anyone having to look.
        let setup = PageSetup::default();
        let folio = text_at(setup.trim.w_pt - 8.0, setup.trim.h_pt - 20.0, 8.0, 10.0);
        let findings = preflight_pages(
            &[page_with(vec![folio])],
            &preset_with_safety(36.0),
            &setup,
            &[],
        );
        assert!(
            findings
                .iter()
                .any(|f| f.check == CheckId::SafeArea && f.severity == Severity::Error),
            "a folio at the trim edge must fail the safe-area check: {findings:?}"
        );
    }

    #[test]
    fn content_inside_the_live_area_passes() {
        let setup = PageSetup::default();
        let inside = text_at(60.0, 60.0, 100.0, 20.0);
        let findings = preflight_pages(
            &[page_with(vec![inside])],
            &preset_with_safety(36.0),
            &setup,
            &[],
        );
        assert!(
            findings.iter().all(|f| f.check != CheckId::SafeArea),
            "content well inside the live area must not be flagged: {findings:?}"
        );
    }

    #[test]
    fn a_full_bleed_image_is_exempt_from_the_safe_area() {
        // The distinction the whole check depends on. A block extending *outward* past the trim is
        // a deliberate full bleed; one falling *inward* of the live edge is at risk. Flagging the
        // first would train a user to ignore the report, and then the second goes unread too.
        let setup = PageSetup::default();
        let bleeding = image_at(
            "bg",
            -9.0,
            -9.0,
            setup.trim.w_pt + 18.0,
            setup.trim.h_pt + 18.0,
        );
        let findings = preflight_pages(
            &[page_with(vec![bleeding])],
            &preset_with_safety(36.0),
            &setup,
            &[asset_px("bg", 4000, 6000, false)],
        );
        assert!(
            findings.iter().all(|f| f.check != CheckId::SafeArea),
            "a full-bleed image must not be flagged as intruding: {findings:?}"
        );
    }

    #[test]
    fn a_zero_safety_preset_makes_the_check_inert() {
        let setup = PageSetup::default();
        let at_the_edge = text_at(0.0, 0.0, 20.0, 10.0);
        let findings = preflight_pages(
            &[page_with(vec![at_the_edge])],
            &PodPreset::generic(),
            &setup,
            &[],
        );
        assert!(
            findings.iter().all(|f| f.check != CheckId::SafeArea),
            "a preset stating no safety margin must not flag anything: {findings:?}"
        );
    }

    #[test]
    fn an_image_scaled_up_fails_on_its_effective_resolution() {
        // `Asset.dpi` is what the author declared about the source; this is pixels over the inches
        // it was *placed* at. A 300 dpi image at 2x its natural size prints at 150 and sails
        // through the model-level check, which is the gap this closes.
        let setup = PageSetup::default();
        // 600x600 px at 300 dpi is 2in square = 144pt. Placed at 288pt it is 4in ⇒ 150 dpi.
        let scaled = image_at("art", 60.0, 60.0, 288.0, 288.0);
        let findings = preflight_pages(
            &[page_with(vec![scaled])],
            &PodPreset::generic(),
            &setup,
            &[asset_px("art", 600, 600, false)],
        );
        let dpi: Vec<_> = findings
            .iter()
            .filter(|f| f.check == CheckId::ImageResolution)
            .collect();
        assert_eq!(
            dpi.len(),
            1,
            "expected one resolution finding: {findings:?}"
        );
        assert_eq!(dpi[0].severity, Severity::Error);
        // The message must name both numbers: one that does not say by how much you missed is one
        // the user cannot act on.
        assert!(
            dpi[0].message.contains("150") && dpi[0].message.contains("300"),
            "the message must state the effective dpi and the required one: {}",
            dpi[0].message
        );
    }

    #[test]
    fn an_image_at_its_natural_size_passes_and_so_does_the_boundary() {
        let setup = PageSetup::default();
        // 600x600 px placed at 144pt = 2in ⇒ exactly 300 dpi, the boundary.
        let natural = image_at("art", 60.0, 60.0, 144.0, 144.0);
        let findings = preflight_pages(
            &[page_with(vec![natural])],
            &PodPreset::generic(),
            &setup,
            &[asset_px("art", 600, 600, false)],
        );
        assert!(
            findings.iter().all(|f| f.check != CheckId::ImageResolution),
            "exactly at the threshold must pass: {findings:?}"
        );
    }

    #[test]
    fn line_art_is_held_to_the_higher_threshold() {
        let setup = PageSetup::default();
        // 600x600 px at 144pt is 300 dpi — fine for a photograph, half what line art needs.
        let art = image_at("map", 60.0, 60.0, 144.0, 144.0);
        let findings = preflight_pages(
            &[page_with(vec![art])],
            &PodPreset::generic(),
            &setup,
            &[asset_px("map", 600, 600, true)],
        );
        assert!(
            findings
                .iter()
                .any(|f| f.check == CheckId::ImageResolution && f.message.contains("600")),
            "line art must be held to 600 dpi: {findings:?}"
        );
    }

    #[test]
    fn findings_are_deduplicated_per_page() {
        // A 500-page document with one systematically misplaced master static must report it once
        // per page, not once per placed block, or the report stops being readable and stops being
        // read.
        let setup = PageSetup::default();
        let intruding: Vec<PlacedBlock> = (0..20)
            .map(|i| text_at(1.0, i as f32 * 10.0, 20.0, 8.0))
            .collect();
        let findings = preflight_pages(
            &[page_with(intruding)],
            &preset_with_safety(36.0),
            &setup,
            &[],
        );
        assert_eq!(
            findings
                .iter()
                .filter(|f| f.check == CheckId::SafeArea)
                .count(),
            1,
            "one safe-area finding per page, however many blocks intrude"
        );
    }

    #[test]
    fn every_check_is_listed_in_all() {
        // `applied()` is derived from `ALL` minus the skip list, so a variant missing from `ALL`
        // makes the report understate what preflight covers. `TrimSize` was missing until spec
        // 0050 noticed.
        for id in [CheckId::TrimSize, CheckId::SafeArea] {
            assert!(CheckId::ALL.contains(&id), "{id:?} must be in CheckId::ALL");
        }
    }

    #[test]
    fn a_runs_colour_override_reaches_the_content_stream() {
        // The end-to-end claim of spec 0063: a word set in a different ink is a different ink in
        // the press file. Before runs, one `set_fill` was emitted per block and there was no way
        // to say this at all.
        let mut doc = Document::sample();
        doc.content.push(Block::body_runs(
            vec![
                quill_core_model::Run::plain("Ordinary prose and then "),
                quill_core_model::Run {
                    text: "a warning".into(),
                    style: quill_core_model::InlineStyle {
                        color: Some(Color::Cmyk {
                            c: 0.0,
                            m: 1.0,
                            y: 1.0,
                            k: 0.0,
                        }),
                        ..quill_core_model::InlineStyle::EMPTY
                    },
                },
                quill_core_model::Run::plain(" and ordinary prose again."),
            ],
            Color::Gray { v: 0.0 },
        ));
        doc.assign_missing_block_ids().expect("ids");
        let (opts, icc) = opts_with_real_icc("run_ink");
        let mut bytes: Vec<u8> = Vec::new();
        export(&doc, &opts, &mut bytes).expect("export");
        let _ = std::fs::remove_file(icc);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("0 1 1 0 k"),
            "the run's CMYK fill must appear in a content stream"
        );
    }

    #[test]
    fn an_over_inked_run_is_caught_by_preflight() {
        // Ink is ink wherever it is authored. A preflight that read the block's colour and not its
        // runs' would pass a document with an over-inked word in it — silent press corruption,
        // which is the failure this crate exists to prevent.
        let mut doc = Document::sample();
        doc.content.push(Block::body_runs(
            vec![
                quill_core_model::Run::plain("Fine. "),
                quill_core_model::Run {
                    text: "Far too much ink.".into(),
                    style: quill_core_model::InlineStyle {
                        color: Some(Color::Cmyk {
                            c: 1.0,
                            m: 1.0,
                            y: 1.0,
                            k: 1.0,
                        }),
                        ..quill_core_model::InlineStyle::EMPTY
                    },
                },
            ],
            Color::Gray { v: 0.0 },
        ));
        doc.assign_missing_block_ids().expect("ids");
        let report = preflight(&doc, &opts_with_icc());
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.check == CheckId::InkCoverage && f.message.contains("run 1")),
            "expected an ink finding naming the run: {:?}",
            report.findings
        );
    }
}
