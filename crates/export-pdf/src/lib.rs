//! Press-ready PDF/X export and preflight. See `specs/0001-pdf-x-export.md`.
//!
//! Preflight (validating a document against the DriveThruRPG print spec) is implemented and
//! tested here. The PDF byte generation itself (via `pdf-writer` + `subsetter`, with CMYK
//! conversion through `lcms2`) lands in a subsequent spec-driven commit; [`export`] currently
//! returns [`ExportError::NotImplemented`] once a document passes preflight.

use std::io::Write;

use quill_color::{within_ink_limit, MAX_INK_COVERAGE_PCT};
use quill_core_model::{Block, Color, Document, DEFAULT_BLEED_PT};
use thiserror::Error;

/// Target PDF/X conformance level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfxVersion {
    /// PDF/X-1a:2001 — CMYK/spot only, no live transparency.
    X1a2001,
    /// PDF/X-3:2002 — allows color-managed content with an output intent.
    X3_2002,
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
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            version: PdfxVersion::X1a2001,
            output_intent_icc: String::new(),
            bleed_pt: DEFAULT_BLEED_PT,
            force: false,
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
    OutputIntent,
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
    #[error("PDF/X byte generation is not implemented yet (see specs/0001-pdf-x-export.md)")]
    NotImplemented,
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
    }

    report
}

/// Export a document as PDF/X. Runs preflight first (unless `opts.force`).
///
/// PDF byte generation is not yet implemented; a clean document therefore reaches
/// [`ExportError::NotImplemented`]. See the module docs and spec 0001.
pub fn export(
    doc: &Document,
    opts: &ExportOptions,
    _out: &mut impl Write,
) -> Result<(), ExportError> {
    if !opts.force {
        let report = preflight(doc, opts);
        if !report.passed() {
            return Err(ExportError::PreflightFailed(report.error_count()));
        }
    }
    Err(ExportError::NotImplemented)
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
            dpi: 299.0,
            line_art: false,
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
            dpi: 400.0,
            line_art: true,
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
    fn export_refuses_when_preflight_fails() {
        let mut sink = Vec::new();
        // Default opts have no ICC -> preflight fails -> export refuses before NotImplemented.
        let e = export(&Document::sample(), &ExportOptions::default(), &mut sink).unwrap_err();
        assert!(matches!(e, ExportError::PreflightFailed(_)));
    }

    #[test]
    fn export_reaches_not_implemented_when_clean() {
        let mut sink = Vec::new();
        let e = export(&Document::sample(), &opts_with_icc(), &mut sink).unwrap_err();
        assert!(matches!(e, ExportError::NotImplemented));
    }
}
