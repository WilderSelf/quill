//! Page geometry: media/bleed and trim boxes for a page.
//!
//! Promoted out of `export-pdf`'s private modules by spec 0032, because the screen renderer needs
//! the same rectangles the exporter does. It lives beside `PageSetup`, which it is derived from.
//!
//! Note the one-source-of-truth rule this repository already learned the hard way (spec 0013): the
//! bleed a validator checks and the bleed a writer emits must come from the same place. There is
//! exactly one `page_geom`, and `export-pdf`'s private copy was deleted rather than left behind.
//!
//! The top-left → bottom-left *flip* deliberately does **not** live here: that is a PDF coordinate
//! convention, not a fact about the page, and it stays in the writer.

use crate::PageSetup;

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
