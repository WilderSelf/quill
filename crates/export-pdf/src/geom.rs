//! The PDF-space coordinate flip.
//!
//! The page *rectangles* moved to `quill-core-model` in spec 0032, so the screen renderer can use
//! the same geometry the exporter does without depending on the PDF pipeline. What stays here is
//! the part that is genuinely about PDF: core-model measures points from the **top-left** of the
//! trim, PDF measures from the **bottom-left** of the page.

pub use quill_core_model::{page_geom, PageGeom};

/// Flip a core-model point (top-left origin, trim space) into PDF space (bottom-left origin,
/// media space).
pub fn flip(g: &PageGeom, x_pt: f32, y_pt: f32) -> (f32, f32) {
    (g.off_x + x_pt, g.media_h - (g.off_y + y_pt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core_model::{PageSetup, Size, DEFAULT_BLEED_PT};

    fn setup(facing: bool) -> PageSetup {
        PageSetup {
            trim: Size {
                w_pt: 432.0,
                h_pt: 648.0,
            },
            bleed_pt: DEFAULT_BLEED_PT,
            facing_pages: facing,
            ..PageSetup::default()
        }
    }

    #[test]
    fn non_facing_bleeds_all_four_edges() {
        let g = page_geom(&setup(false), 0);
        assert_eq!(g.media_w, 432.0 + 18.0);
        assert_eq!(g.media_h, 648.0 + 18.0);
        assert_eq!(g.off_x, 9.0);
        assert_eq!(g.off_y, 9.0);
        // Trim centered: bottom-left at (9, 9).
        assert_eq!(g.trim_origin_pdf(), (9.0, 9.0));
    }

    #[test]
    fn facing_recto_binds_left_no_left_bleed() {
        let g = page_geom(&setup(true), 0); // recto
        assert_eq!(g.media_w, 432.0 + 9.0); // only one horizontal bleed
        assert_eq!(g.off_x, 0.0); // trim flush to media left (binding edge)
        let (tx, _ty) = g.trim_origin_pdf();
        assert_eq!(tx, 0.0);
    }

    #[test]
    fn facing_verso_binds_right() {
        let g = page_geom(&setup(true), 1); // verso
        assert_eq!(g.media_w, 432.0 + 9.0);
        assert_eq!(g.off_x, 9.0); // bleed on the left, binding on the right
    }

    #[test]
    fn binding_edge_bleed_is_asymmetric_between_facing_pages() {
        // The recto and verso pages place the trim on opposite sides — this is the asymmetry
        // spec 0002's acceptance criteria checks.
        let recto = page_geom(&setup(true), 0);
        let verso = page_geom(&setup(true), 1);
        assert_ne!(recto.off_x, verso.off_x);
        // Non-facing has no such asymmetry.
        let a = page_geom(&setup(false), 0);
        let b = page_geom(&setup(false), 1);
        assert_eq!(a.off_x, b.off_x);
    }

    #[test]
    fn flip_maps_top_left_to_bottom_left() {
        let g = page_geom(&setup(false), 0);
        // A point at the very top-left of the trim (0,0) maps to trim's top in PDF space.
        let (x, y) = flip(&g, 0.0, 0.0);
        assert_eq!(x, 9.0);
        assert_eq!(y, g.media_h - 9.0); // near the top of the page
    }
}
