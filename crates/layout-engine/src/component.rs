//! The component interpreter (spec 0054).
//!
//! One function lays out *any* [`ComponentDef`]. Before this, `measure_stat_block` and
//! `measure_table` did the same job twice, with every tint, inset, hairline and section order
//! written as a crate constant — so a publisher whose game system sets its creatures differently
//! could not express it at all.
//!
//! The acceptance criterion for the generalization is that it moves no geometry: the two bundled
//! components, re-expressed as definitions, produce byte-identical `PlacedBlock` output. Where a
//! rule below looks arbitrary it is because it is faithful — the shipped behaviour is the
//! specification, and a "tidier" variant of it is a silent change to every book already laid out.

use quill_core_model::{
    normalized_widths, Color, ComponentDef, ComponentFields, DefColor, DefStroke, FieldValue,
    ParagraphStyle, RuleDef, SectionShape, SplitGranularity, StyleSheet,
};
use quill_text_layout::{justify_paragraph_indented, Alignment, Hyphenator, Line, RunMetrics};

use crate::{Measured, PanelPart, PanelRect, PanelSplit, Stroke};

fn ink(c: DefColor) -> Color {
    match c {
        DefColor::Gray { v } => Color::Gray { v },
        DefColor::Cmyk { c, m, y, k } => Color::Cmyk { c, m, y, k },
    }
}

fn stroke(s: DefStroke) -> Stroke {
    Stroke {
        color: ink(s.color),
        width_pt: s.width_pt,
    }
}

/// Lay out one instance of `def`.
///
/// Returns the placement payload and the component's height, exactly as the two functions it
/// replaces did.
pub(crate) fn measure_component(
    def: &ComponentDef,
    fields: &ComponentFields,
    color: Color,
    width: f32,
    styles: &StyleSheet,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> (Measured, f32) {
    let padding = def.panel.padding_pt;
    let inner_w = (width - padding * 2.0).max(1.0);

    // Columns are shared across every `rows` section, so a header and its body agree on where a
    // column starts. Deriving the count per section would let a header with three cells and a body
    // with four disagree, which is a table that does not line up.
    let column_count = def
        .sections
        .iter()
        .filter(|s| matches!(s.shape, SectionShape::Rows { .. }))
        .flat_map(|s| {
            fields
                .get(&s.source)
                .map(FieldValue::as_rows)
                .unwrap_or(&[])
        })
        .map(Vec::len)
        .max()
        .unwrap_or(0);

    let mut parts: Vec<PanelPart> = Vec::new();
    let mut decorations: Vec<PanelRect> = Vec::new();
    let mut y = padding;

    // The prefix every continuation re-states (spec 0045): everything laid up to and including the
    // last `repeat` section. With no repeated section it is the panel's own top inset, which is
    // what a cut stat block re-states so its text never sits on the rule.
    let mut repeat_h = padding;
    let mut repeat_parts: Vec<PanelPart> = Vec::new();
    let mut repeat_decorations: Vec<PanelRect> = Vec::new();

    // Absolute panel-local y of each place the component may be cut. Item 0 is rewritten to 0.0
    // once the walk is done, so it absorbs `repeat_h` — the invariant `PanelSplit::items` states.
    let mut boundaries: Vec<f32> = Vec::new();
    let mut emitted_section = false;

    let by_element = def.split.granularity == SplitGranularity::Elements;
    let draw_rule = |rule: &RuleDef, y: &mut f32, decorations: &mut Vec<PanelRect>| {
        *y += rule.gap_above_pt;
        let (dx, w) = if rule.full_width {
            (0.0, width)
        } else {
            (padding, inner_w)
        };
        decorations.push(PanelRect {
            dx_pt: dx,
            dy_pt: *y,
            w_pt: w,
            h_pt: rule.thickness_pt,
            fill: Some(ink(rule.color)),
            stroke: None,
        });
        // The thickness does not advance the cursor; the gap below does. See `RuleDef`.
        *y += rule.gap_below_pt;
    };

    for section in &def.sections {
        let style: ParagraphStyle = styles
            .paragraph
            .get(section.style.as_str())
            .copied()
            .unwrap_or_default();
        let value = fields.get(section.source.as_str());

        match &section.shape {
            SectionShape::Rows {
                widths,
                cell_padding_pt,
                zebra,
            } => {
                let rows = value.map(FieldValue::as_rows).unwrap_or(&[]);
                if rows.is_empty() || column_count == 0 {
                    continue;
                }
                let fractions = normalized_widths(
                    widths
                        .as_deref()
                        .and_then(|f| fields.get(f))
                        .map(FieldValue::as_widths)
                        .unwrap_or(&[]),
                    column_count,
                );
                // Column x offsets and widths, in points, with the cell padding taken off the
                // *measure* so a wrapped cell stays inside its column rather than being broken wide
                // and then drawn inset.
                let mut x = padding;
                let mut columns: Vec<(f32, f32)> = Vec::with_capacity(column_count);
                for f in &fractions {
                    let w = inner_w * f;
                    columns.push((x + cell_padding_pt, (w - cell_padding_pt * 2.0).max(1.0)));
                    x += w;
                }

                let banding_on = zebra
                    .as_ref()
                    .map(|z| {
                        z.enabled_by
                            .as_deref()
                            .and_then(|f| fields.get(f))
                            .map(FieldValue::as_flag)
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);

                // A `repeat` section records no cut boundary: it is the prefix every fragment
                // begins with, not content that can be left behind. Charging it a boundary would
                // hand item 0 the prefix *alone* and the first row its own item, overfilling every
                // continuation by exactly one row.
                if !section.repeat {
                    boundaries.push(y);
                }
                if emitted_section {
                    if let Some(rule) = &section.rule_above {
                        draw_rule(rule, &mut y, &mut decorations);
                    }
                }
                for (i, row) in rows.iter().enumerate() {
                    let band_top = y;
                    if i > 0 && by_element && !section.repeat {
                        boundaries.push(band_top);
                    }
                    // A row's height is its tallest cell: a wrapped cell must push the row down,
                    // not overlap the one beneath it. A `rows` section takes its vertical spacing
                    // from the cell padding, not from the style's space before/after — the inset is
                    // the mechanism, and applying both would double it.
                    let mut row_h: f32 = style.leading_pt;
                    for (c, cell) in row.iter().enumerate().take(column_count) {
                        let (cx, cw) = columns[c];
                        let lines = break_run(cell, cw, style, metrics, hyphenator);
                        row_h = row_h.max(lines.len() as f32 * style.leading_pt);
                        parts.push(PanelPart {
                            dx_pt: cx,
                            dy_pt: y + cell_padding_pt,
                            w_pt: cw,
                            lines,
                            color,
                            font_size_pt: style.font_size_pt,
                            leading_pt: style.leading_pt,
                            link_page: None,
                        });
                    }
                    let h = row_h + cell_padding_pt * 2.0;
                    y += h;
                    if banding_on {
                        let parity = zebra.as_ref().map(|z| z.parity).unwrap_or(1);
                        if i % 2 == parity % 2 {
                            // Behind the row's text: `decorations` are emitted before `parts`, so
                            // paint order is structural rather than something each caller
                            // remembers.
                            decorations.push(PanelRect {
                                dx_pt: 0.0,
                                dy_pt: band_top,
                                w_pt: width,
                                h_pt: h,
                                fill: Some(ink(zebra.as_ref().unwrap().fill)),
                                stroke: None,
                            });
                        }
                    }
                }
            }
            shape => {
                let runs = text_runs(shape, value);
                if runs.is_empty() {
                    continue;
                }
                for (i, text) in runs.iter().enumerate() {
                    if !section.repeat && (i == 0 || by_element) {
                        // Recorded before the section's space-above, so a fragment that begins here
                        // begins with that space rather than swallowing it.
                        boundaries.push(y);
                    }
                    y += style.space_before_pt;
                    if i == 0 && emitted_section {
                        // The first *emitted* section never draws its rule: it has the panel's own
                        // edge above it. An absent `overview` must not leave a hairline behind.
                        if let Some(rule) = &section.rule_above {
                            draw_rule(rule, &mut y, &mut decorations);
                        }
                    }
                    let lines = break_run(text, inner_w, style, metrics, hyphenator);
                    let n = lines.len();
                    parts.push(PanelPart {
                        dx_pt: padding,
                        dy_pt: y,
                        w_pt: inner_w,
                        lines,
                        color,
                        font_size_pt: style.font_size_pt,
                        leading_pt: style.leading_pt,
                        link_page: None,
                    });
                    y += n as f32 * style.leading_pt + style.space_after_pt;
                }
            }
        }

        if let Some(rule) = &section.rule_below {
            draw_rule(rule, &mut y, &mut decorations);
        }
        if section.repeat {
            repeat_h = y;
            repeat_parts = parts.clone();
            repeat_decorations = decorations.clone();
        }
        emitted_section = true;
    }

    if parts.is_empty() && decorations.is_empty() {
        // An empty component occupies nothing rather than drawing an empty box.
        return (
            Measured::Panel {
                fill: None,
                stroke: None,
                parts: Vec::new(),
                decorations: Vec::new(),
                split: None,
            },
            0.0,
        );
    }

    // Item 0 absorbs the repeated prefix and the panel's top inset alike.
    if let Some(first) = boundaries.first_mut() {
        *first = 0.0;
    }
    let split = match def.split.granularity {
        SplitGranularity::Whole => None,
        _ => {
            let mut items: Vec<f32> = Vec::with_capacity(boundaries.len());
            for i in 0..boundaries.len() {
                let end = boundaries.get(i + 1).copied().unwrap_or(y);
                items.push(end - boundaries[i]);
            }
            Some(PanelSplit {
                items,
                repeat_parts,
                repeat_decorations,
                repeat_h,
                // Space a fragment reserves below its last item, so a cut panel closes with the
                // same inset it opens with.
                trailing_pt: padding,
                min_items: def.split.min_items.max(1),
                keep_together: def.split.keep_together,
            })
        }
    };

    (
        Measured::Panel {
            fill: def.panel.fill.map(ink),
            stroke: def.panel.stroke.map(stroke),
            parts,
            decorations,
            split,
        },
        y + padding,
    )
}

/// Break one run through the same Knuth-Plass path body text goes through, honouring the style's
/// indent (spec 0048) — which is what lets `statblock-attr` hang a wrapped value under the value
/// rather than under the key.
fn break_run(
    text: &str,
    width: f32,
    style: ParagraphStyle,
    metrics: &impl RunMetrics,
    hyphenator: &impl Hyphenator,
) -> Vec<Line> {
    justify_paragraph_indented(
        text,
        width,
        quill_text_layout::Indent {
            first_pt: style.indent.first_pt,
            rest_pt: style.indent.rest_pt,
        },
        style.font_size_pt,
        // Ragged, never justified: a component sits in a narrow measure where justification opens
        // rivers the surrounding body text would not show.
        Alignment::Left,
        metrics,
        hyphenator,
    )
}

/// The runs a non-`rows` section emits.
fn text_runs(shape: &SectionShape, value: Option<&FieldValue>) -> Vec<String> {
    match shape {
        // Always one run, even when the field is missing: a component's title is what opens it, and
        // a nameless one should still be a panel rather than nothing.
        SectionShape::Text => vec![value.map(FieldValue::as_text).unwrap_or("").to_string()],
        SectionShape::Lines => value
            .map(FieldValue::as_lines)
            .unwrap_or(&[])
            .iter()
            .map(String::clone)
            .collect(),
        SectionShape::Pairs { separator } => value
            .map(FieldValue::as_pairs)
            .unwrap_or(&[])
            .iter()
            .map(|(k, v)| {
                // The key's own words are joined by U+00A0, so `Armour Class:` is one unbreakable
                // box (spec 0048). Before that, a ~150 pt column broke after `Armour` and the
                // key/value pairing was lost — found by rendering spec 0038's panel, not by any
                // assertion.
                let key = k.split_whitespace().collect::<Vec<_>>().join("\u{a0}");
                format!("{key}{separator}{v}")
            })
            .collect(),
        SectionShape::Rows { .. } => Vec::new(),
    }
}
