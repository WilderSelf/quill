//! POD presets — the printer's requirements as data. See `specs/0049-pod-presets.md`.
//!
//! Everything preflight checks used to be one vendor's numbers, hard-coded: 240% ink, 300/600 dpi,
//! 9 pt bleed, PDF/X-1a. [`PodPreset`] makes them a named, auditable bundle, and adds the number
//! quill never had — a **safety margin**, the distance inside the trim within which content is at
//! risk from the guillotine. Nothing reads the safety margin yet; spec 0050 adds the check.
//!
//! **Vendor presets are a convenience to be confirmed against the vendor's current specification,
//! not a warranty.** A preset that misstates a vendor's requirement is worse than no preset because
//! it looks authoritative — `CLAUDE.md`'s "prefer a visible failure over silent press-corruption"
//! applied to data. Hence [`PodPreset::confirmed`], the mandatory [`PodPreset::source`] /
//! [`PodPreset::retrieved`] provenance, and a bundled catalogue that fills what it could not verify
//! with `generic`'s conservative values rather than with plausible-looking invented ones.

use quill_core_model::{Pt, Size};

use crate::PdfxVersion;

/// One inch in points; the unit every press number below is quoted in.
const IN_PT: Pt = 72.0;

/// The requirements one print-on-demand vendor states, as data.
///
/// Constructed rather than parsed: a preset is code-shaped today, and the file format deliberately
/// does **not** carry one (a document is not bound to one printer — see `docs/format-spec.md`).
/// Every field is public so a user with their printer's real specification can state it directly.
#[derive(Debug, Clone, PartialEq)]
pub struct PodPreset {
    /// CLI name (`--preset <name>`), lowercase and stable.
    pub name: String,
    /// Human-readable name for listings.
    pub title: String,
    /// **Where these numbers came from.** Read it before trusting them. Never empty.
    pub source: String,
    /// The date `source` was consulted, `YYYY-MM-DD`. Never empty. Vendor requirements change, and
    /// a preset that cannot be audited against its source becomes wrong silently.
    pub retrieved: String,
    /// True only when the numbers were actually read from `source`. A `false` preset carries
    /// `generic`'s conservative values as placeholders and says so in `source`; the CLI announces
    /// it on stderr when one is selected.
    pub confirmed: bool,
    /// The trims this preset states. **Empty means it states none**, and the trim check is then
    /// inert — quill will not put a catalogue in a vendor's mouth, and warning about a size the
    /// vendor may well offer is worse than saying nothing.
    pub trim_sizes: Vec<Size>,
    /// Required bleed on the three non-binding edges.
    pub bleed_pt: Pt,
    /// Content nearer the trim than this is at risk from trimming. `0.0` means the preset states no
    /// margin, which makes spec 0050's safe-area check inert.
    pub safety_pt: Pt,
    /// Maximum total ink coverage, percent (C+M+Y+K).
    pub max_ink_pct: f32,
    /// Minimum resolution for continuous-tone images.
    pub min_dpi_color: f32,
    /// Minimum resolution for bilevel line art.
    pub min_dpi_line_art: f32,
    /// The PDF/X conformance level this vendor wants.
    pub pdfx: PdfxVersion,
}

impl Default for PodPreset {
    fn default() -> Self {
        Self::generic()
    }
}

impl PodPreset {
    /// The conservative default, numerically identical to the constants it replaced.
    ///
    /// 9 pt bleed, 240% ink, 300/600 dpi and PDF/X-1a:2001 are quill's own press constraints, so
    /// adding presets changes no existing behaviour — asserted field-by-field against
    /// `quill_color::MAX_INK_COVERAGE_PCT` and `quill_core_model::DEFAULT_BLEED_PT` in
    /// `tests/presets.rs`, and end-to-end by the golden preflight report in the `cli` crate.
    ///
    /// Two fields have no pre-0049 predecessor and are therefore stated for what they are:
    ///
    /// - `safety_pt` is **0.0 — `generic` states no safety margin**, for the same reason the vendor
    ///   presets state no trim catalogue: no figure has been confirmed against a printer, and quill
    ///   will not invent a requirement. That makes spec 0050's safe-area check inert by default;
    ///   set it from your printer's specification to turn it on. This is not a nominal default
    ///   either: `PageSetup::default()` has zero margins on purpose (spec 0036 gives templates real
    ///   ones instead), so a non-zero figure here would fail the shipped sample — correctly, which
    ///   is precisely why the number has to come from a printer and not from quill.
    /// - `trim_sizes` is a short list of common trade trims — a convenience, explicitly not a
    ///   vendor catalogue. A trim outside it is a `Warning`, never an `Error`.
    pub fn generic() -> Self {
        Self {
            name: "generic".into(),
            title: "Generic POD (conservative)".into(),
            source: "quill's own press constraints — specs/0001-pdf-x-export.md and CLAUDE.md: \
                     0.125in bleed, 300 dpi images (600 dpi line art), 240% total ink, \
                     PDF/X-1a:2001. No safety margin is stated — supply your printer's figure to \
                     vendor's stated requirement. The trim list is common trade sizes, not a \
                     vendor catalogue."
                .into(),
            retrieved: "2026-07-27".into(),
            confirmed: true,
            trim_sizes: vec![
                inches(6.0, 9.0),  // digest — quill's own PageSetup::default
                inches(5.5, 8.5),  // trade paperback
                inches(7.0, 10.0), // small RPG hardback
                inches(8.5, 11.0), // US Letter
                Size {
                    w_pt: 595.0,
                    h_pt: 842.0,
                }, // A4
                Size {
                    w_pt: 420.0,
                    h_pt: 595.0,
                }, // A5
            ],
            bleed_pt: 0.125 * IN_PT,
            safety_pt: 0.0,
            max_ink_pct: 240.0,
            min_dpi_color: 300.0,
            min_dpi_line_art: 600.0,
            pdfx: PdfxVersion::X1a2001,
        }
    }

    /// Every bundled preset, `generic` first.
    ///
    /// The three vendor entries carry `generic`'s numbers because the vendors' current published
    /// specifications were **not** re-read when this shipped. Rather than invent plausible values,
    /// each says so in `source`, reports `confirmed: false`, and states no trim catalogue. They are
    /// named slots, filled conservatively, that announce that they are.
    pub fn bundled() -> Vec<PodPreset> {
        vec![
            Self::generic(),
            Self::unconfirmed_vendor(
                "drivethrurpg",
                "DriveThruRPG (UNCONFIRMED)",
                "NOT re-read from DriveThruRPG's current print guidelines. Every value is \
                 `generic`'s — which quill's own spec 0001 attributes to DriveThruRPG, but that \
                 attribution has not been re-verified against the vendor. Treat as unconfirmed and \
                 check against DriveThruRPG's current specification before relying on it. No trim \
                 catalogue is stated.",
            ),
            Self::unconfirmed_vendor(
                "lulu",
                "Lulu (UNCONFIRMED)",
                "NOT confirmed against Lulu's published specification. Every value is `generic`'s \
                 conservative default and none of them is a statement of Lulu's requirements — \
                 this preset is a named slot to be filled in from Lulu's current spec. No trim \
                 catalogue is stated.",
            ),
            Self::unconfirmed_vendor(
                "ingramspark",
                "IngramSpark (UNCONFIRMED)",
                "NOT confirmed against IngramSpark's published specification. Every value is \
                 `generic`'s conservative default and none of them is a statement of IngramSpark's \
                 requirements — this preset is a named slot to be filled in from IngramSpark's \
                 current spec. No trim catalogue is stated.",
            ),
        ]
    }

    /// A vendor slot whose numbers were not verified: `generic`'s values, no trim catalogue, and a
    /// `source` that says so.
    fn unconfirmed_vendor(name: &str, title: &str, source: &str) -> PodPreset {
        PodPreset {
            name: name.into(),
            title: title.into(),
            source: source.into(),
            retrieved: "2026-07-27".into(),
            confirmed: false,
            // Deliberately empty: an unstated catalogue warns about nothing, which is the honest
            // outcome when the vendor's real list is unknown.
            trim_sizes: Vec::new(),
            ..Self::generic()
        }
    }

    /// Look a preset up by CLI name. `None` for an unknown name — callers must report
    /// [`PodPreset::names`] rather than fall back to a default, since silently printing to the
    /// wrong vendor's numbers is the failure this whole increment exists to prevent.
    pub fn by_name(name: &str) -> Option<PodPreset> {
        Self::bundled().into_iter().find(|p| p.name == name)
    }

    /// The bundled names, in listing order — what an unknown-name error prints.
    pub fn names() -> Vec<String> {
        Self::bundled().into_iter().map(|p| p.name).collect()
    }

    /// The minimum resolution for an asset, by kind.
    pub fn min_dpi(&self, line_art: bool) -> f32 {
        if line_art {
            self.min_dpi_line_art
        } else {
            self.min_dpi_color
        }
    }

    /// Whether `trim` is one this preset lists.
    ///
    /// A preset stating **no** catalogue accepts every trim: an empty list is "unknown", not
    /// "nothing is allowed". Compared with a 0.5 pt tolerance because trims are `f32` derived from
    /// inch or millimetre arithmetic, and a 6×9 that arrives as 431.99999 pt is a 6×9.
    pub fn lists_trim(&self, trim: Size) -> bool {
        if self.trim_sizes.is_empty() {
            return true;
        }
        self.trim_sizes
            .iter()
            .any(|s| (s.w_pt - trim.w_pt).abs() < 0.5 && (s.h_pt - trim.h_pt).abs() < 0.5)
    }
}

/// A trim size given in inches.
fn inches(w: f32, h: f32) -> Size {
    Size {
        w_pt: w * IN_PT,
        h_pt: h * IN_PT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_name_is_none_and_the_names_are_listable() {
        // The CLI turns this into an error naming the alternatives; the one thing it must never do
        // is fall back to a default preset, which would print to the wrong vendor's numbers.
        assert!(PodPreset::by_name("no-such-printer").is_none());
        assert!(PodPreset::names().contains(&"generic".to_string()));
        assert!(PodPreset::names().contains(&"lulu".to_string()));
        assert_eq!(PodPreset::by_name("generic"), Some(PodPreset::generic()));
        assert_eq!(PodPreset::default(), PodPreset::generic());
    }

    #[test]
    fn a_preset_stating_no_trim_catalogue_accepts_every_trim() {
        // "Unknown", not "nothing allowed" — the difference between a useless warning on every
        // document and an honest silence.
        let lulu = PodPreset::by_name("lulu").expect("bundled");
        assert!(lulu.trim_sizes.is_empty());
        assert!(lulu.lists_trim(Size {
            w_pt: 123.0,
            h_pt: 456.0
        }));

        let generic = PodPreset::generic();
        assert!(generic.lists_trim(Size {
            w_pt: 432.0,
            h_pt: 648.0
        }));
        assert!(!generic.lists_trim(Size {
            w_pt: 123.0,
            h_pt: 456.0
        }));
    }

    #[test]
    fn min_dpi_splits_line_art_from_continuous_tone() {
        let p = PodPreset::generic();
        assert_eq!(p.min_dpi(false), 300.0);
        assert_eq!(p.min_dpi(true), 600.0);
    }
}
