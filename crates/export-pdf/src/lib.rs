//! Press-ready PDF/X export and preflight. See `specs/0001-pdf-x-export.md` (preflight) and
//! `specs/0002-pdf-byte-generation.md` (byte generation).
//!
//! [`preflight`] validates a document against the DriveThruRPG/PDF-X requirements. [`export`]
//! then writes a real **PDF/X-1a:2001** or **PDF/X-3:2002** file (selected via
//! [`ExportOptions::version`]) through `pdf-writer` (object graph) + `subsetter` (embedded subset
//! font), with `lcms2` validating the ICC OutputIntent. The writer internals live in the
//! `writer`/`fonts`/`images`/`icc`/`xmp`/`geom` modules.

use std::io::Write;
use std::path::PathBuf;

use quill_color::{within_ink_limit, MAX_INK_COVERAGE_PCT};
use quill_core_model::{Block, Color, Document, DEFAULT_BLEED_PT};
use quill_layout_engine::{LaidOutPage, PlacedBlock};
use thiserror::Error;

mod fonts;
mod geom;
mod hyphenate;
mod icc;
mod images;
mod writer;
mod xmp;

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

/// Options controlling an export.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub version: PdfxVersion,
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
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            version: PdfxVersion::X1a2001,
            output_intent_icc: String::new(),
            force: false,
            font_path: None,
            // `.` preserves the pre-spec-0025 behavior for callers that never set it, so this
            // change cannot move where an existing caller's images resolve from.
            asset_root: PathBuf::from("."),
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

/// The outcome of preflighting a document.
#[derive(Debug, Clone, Default)]
pub struct PreflightReport {
    pub findings: Vec<Finding>,
}

impl PreflightReport {
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

fn min_dpi(line_art: bool) -> f32 {
    if line_art {
        600.0
    } else {
        300.0
    }
}

/// Validate a document against the PDF/X / DriveThruRPG requirements from spec 0001.
pub fn preflight(doc: &Document, opts: &ExportOptions) -> PreflightReport {
    let mut report = PreflightReport::default();

    // Colors: no RGB in output; every color within the ink limit.
    for (i, block) in doc.content.iter().enumerate() {
        let color = match block {
            Block::Heading { color, .. }
            | Block::Body { color, .. }
            | Block::StatBlock { color, .. } => Some(color),
            Block::Image { .. } => None,
        };
        let Some(color) = color else { continue };
        if matches!(color, Color::Rgb { .. }) {
            push_error(
                &mut report,
                CheckId::ColorSpace,
                format!("block {i} uses RGB; press output must be CMYK or grayscale"),
            );
        } else if !within_ink_limit(color) {
            push_error(
                &mut report,
                CheckId::InkCoverage,
                format!("block {i} exceeds {MAX_INK_COVERAGE_PCT}% total ink coverage"),
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

    // Bleed must be at least the required 0.125in on outside edges. Validate the document's own
    // `page_setup.bleed_pt` — the exact value `geom::page_geom` writes into the BleedBox — so
    // preflight rejects the geometry export actually produces (spec 0013).
    let bleed_pt = doc.page_setup.bleed_pt;
    if bleed_pt + f32::EPSILON < DEFAULT_BLEED_PT {
        push_error(
            &mut report,
            CheckId::Bleed,
            format!("bleed {bleed_pt}pt is below the required {DEFAULT_BLEED_PT}pt"),
        );
    }

    // Image resolution.
    for asset in &doc.assets {
        let needed = min_dpi(asset.line_art);
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

    // An ICC OutputIntent is required for PDF/X.
    if opts.output_intent_icc.trim().is_empty() {
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
    // Real en-US hyphenation (spec 0018 incr. 2): words break at legal syllable points, tightening
    // lines and splitting over-wide words. Built once (stateless), passed to the layout pass.
    let hyphenator = hyphenate::HypherHyphenator;
    let pages = quill_layout_engine::lay_out(doc, &shaper, &hyphenator);

    // Colour checks on the *model* cannot see geometry the engine synthesized (spec 0037). A
    // tinted panel or a rule is ink like any other, but it is not a `Block` — so the checks above,
    // which walk `doc.content`, are blind to it. Run the geometry-level checks on the pages the
    // writer is about to draw, rather than laying the document out a second time inside
    // `preflight`.
    if !opts.force {
        let findings = preflight_pages(&pages);
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
pub fn preflight_pages(pages: &[LaidOutPage]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for page in pages {
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
                } else if !within_ink_limit(&color) {
                    findings.push(Finding {
                        check: CheckId::InkCoverage,
                        severity: Severity::Error,
                        message: format!(
                            "page {}: a decoration {what} exceeds {MAX_INK_COVERAGE_PCT}% total \
                             ink coverage",
                            page.index + 1
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
            Block::Heading { text, .. } | Block::Body { text, .. } => set.extend(text.chars()),
            // Spec 0026's silent-failure case, and the reason every variant-adding increment
            // carries a non-ASCII export test: a character this collector misses is not an error
            // anywhere, it just renders as `.notdef` in the finished PDF.
            Block::StatBlock { stat, .. } => {
                set.extend(stat.name.chars());
                for (k, v) in &stat.attributes {
                    set.extend(k.chars());
                    set.extend(v.chars());
                }
                for section in [
                    &stat.overview,
                    &stat.details,
                    &stat.actions,
                    &stat.reactions,
                ] {
                    for line in section {
                        set.extend(line.chars());
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

    #[test]
    fn a_stat_blocks_glyphs_reach_the_font_subset() {
        // Spec 0026's silent-failure case, and the reason every variant-adding increment carries
        // this test: a character `collect_doc_chars` misses is not an error anywhere in the
        // pipeline — it renders as a `.notdef` box in the finished PDF. Every section of a stat
        // block is checked, because each is a separate place the collector could have forgotten.
        let mut doc = Document::sample();
        doc.content.push(Block::StatBlock {
            id: quill_core_model::BlockId::UNASSIGNED,
            stat: quill_core_model::StatBlock {
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
        doc.content.push(Block::StatBlock {
            id: quill_core_model::BlockId::UNASSIGNED,
            stat: quill_core_model::StatBlock {
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
        let findings = preflight_pages(&[page_with_rect(Some(over), None)]);
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
        let by_fill = preflight_pages(&[page_with_rect(Some(rgb), None)]);
        assert_eq!(by_fill.len(), 1);
        assert_eq!(by_fill[0].check, CheckId::ColorSpace);

        let by_stroke = preflight_pages(&[page_with_rect(
            None,
            Some(quill_layout_engine::Stroke {
                color: rgb,
                width_pt: 0.5,
            }),
        )]);
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
        assert!(preflight_pages(&[page_with_rect(Some(tint), None)]).is_empty());
        assert!(preflight_pages(&[]).is_empty());
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
        let findings = preflight_pages(&[page_with_rect(Some(over), None)]);
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
    /// Changed by spec 0038: `StyleSheet::default()` gained the three built-in `statblock-*`
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
    const SAMPLE_EXPORT_DIGEST: u64 = 0x228f_8f78_c1fc_fd8e;

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
}
