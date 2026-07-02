//! Press-ready PDF/X export and preflight. See `specs/0001-pdf-x-export.md` (preflight) and
//! `specs/0002-pdf-byte-generation.md` (byte generation).
//!
//! [`preflight`] validates a document against the DriveThruRPG/PDF-X requirements. [`export`]
//! then writes a real **PDF/X-1a:2001** or **PDF/X-3:2002** file (selected via
//! [`ExportOptions::version`]) through `pdf-writer` (object graph) + `subsetter` (embedded subset
//! font), with `lcms2` validating the ICC OutputIntent. The writer internals live in the
//! `writer`/`fonts`/`images`/`icc`/`xmp`/`geom` modules.

use std::io::Write;

use quill_color::{within_ink_limit, MAX_INK_COVERAGE_PCT};
use quill_core_model::{Block, Color, Document, DEFAULT_BLEED_PT};
use thiserror::Error;

mod fonts;
mod geom;
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
    pub bleed_pt: f32,
    /// Export even if preflight fails.
    pub force: bool,
    /// Path to a user-supplied TrueType (`.ttf`) or CFF OpenType (`.otf`) font to embed. `None`
    /// embeds the bundled Source Serif 4. See specs 0004 (user fonts) and 0011 (CFF).
    pub font_path: Option<String>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            version: PdfxVersion::X1a2001,
            output_intent_icc: String::new(),
            bleed_pt: DEFAULT_BLEED_PT,
            force: false,
            font_path: None,
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
            Block::Heading { color, .. } | Block::Body { color, .. } => Some(color),
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

    // Bleed must be at least the required 0.125in on outside edges.
    if opts.bleed_pt + f32::EPSILON < DEFAULT_BLEED_PT {
        push_error(
            &mut report,
            CheckId::Bleed,
            format!(
                "bleed {}pt is below the required {DEFAULT_BLEED_PT}pt",
                opts.bleed_pt
            ),
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
    let pages = quill_layout_engine::lay_out(doc);
    writer::write_pdf(doc, opts, &pages, out)
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
        doc.content.push(Block::Body {
            text: "oops".into(),
            color: Color::Rgb {
                r: 1.0,
                g: 0.0,
                b: 0.0,
            },
        });
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
        doc.content.push(Block::Image {
            asset: "pic".into(),
        });

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
        doc.content.push(Block::Image {
            asset: "pic".into(),
        });

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
        doc.content.push(Block::Image {
            asset: "pic".into(),
        });

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
