//! Page geometry for PDF/X export: bleed/trim boxes and the top-left → bottom-left flip.
//!
//! core-model measures points from the **top-left** of the trim; PDF measures from the
//! **bottom-left** of the page (here, the bleed box). This module centralizes both the box
//! rectangles (spec 0002 req 6) and the coordinate transform (writer §3).

use quill_core_model::PageSetup;

/// Resolved geometry for a single page, in PDF points (origin = bleed-box bottom-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeom {
    /// Full media/bleed box size (`MediaBox == BleedBox`).
    pub media_w: f32,
    pub media_h: f32,
    /// Trim size.
    pub trim_w: f32,
    pub trim_h: f32,
    /// Offset of the trim's left/top edge from the media's left/top edge, in points.
    pub off_x: f32,
    pub off_y: f32,
}

impl PageGeom {
    /// `TrimBox` bottom-left corner in PDF coordinates.
    pub fn trim_origin_pdf(&self) -> (f32, f32) {
        // Top/bottom edges always bleed, so the trim's bottom sits `off_y` above the media
        // bottom (top and bottom insets are equal for a non-binding vertical axis).
        let bottom = self.media_h - self.off_y - self.trim_h;
        (self.off_x, bottom)
    }

    /// Flip a core-model point (top-left origin, trim space) into PDF space (bottom-left origin,
    /// media space).
    pub fn flip(&self, x_pt: f32, y_pt: f32) -> (f32, f32) {
        (self.off_x + x_pt, self.media_h - (self.off_y + y_pt))
    }
}

/// Compute [`PageGeom`] for the page at `page_index`.
///
/// Vertical edges (top/bottom) always bleed. The horizontal binding edge only exists for a
/// facing-pages document: even indices are recto (binding on the left), odd indices verso
/// (binding on the right); the binding edge gets zero bleed. Non-facing documents bleed all
/// four edges.
pub fn page_geom(setup: &PageSetup, page_index: usize) -> PageGeom {
    let bleed = setup.bleed_pt;
    let trim_w = setup.trim.w_pt;
    let trim_h = setup.trim.h_pt;

    // Top and bottom always bleed.
    let off_y = bleed;
    let media_h = trim_h + 2.0 * bleed;

    let (off_x, media_w) = if setup.facing_pages {
        // One horizontal edge is the binding (no bleed); the outer edge bleeds.
        let media_w = trim_w + bleed;
        if page_index.is_multiple_of(2) {
            // Recto (right-hand): binding on the left → trim flush to media left, bleed on right.
            (0.0, media_w)
        } else {
            // Verso (left-hand): binding on the right → bleed on the left.
            (bleed, media_w)
        }
    } else {
        // Bleed on all four edges.
        (bleed, trim_w + 2.0 * bleed)
    };

    PageGeom {
        media_w,
        media_h,
        trim_w,
        trim_h,
        off_x,
        off_y,
    }
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
        let (x, y) = g.flip(0.0, 0.0);
        assert_eq!(x, 9.0);
        assert_eq!(y, g.media_h - 9.0); // near the top of the page
    }
}
