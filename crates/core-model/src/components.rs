//! The two bundled component definitions (spec 0054).
//!
//! These live in `core-model` rather than beside [`ComponentDef`] itself because they resolve the
//! *document's* style names — `statblock-title`, `table-cell` — and those constants are the
//! document's, not the portable component's. A definition is only ever a set of names; what they
//! resolve to is a stylesheet's business.
//!
//! Every number here was a crate constant or a literal in `layout-engine`'s two measurement
//! functions before this spec. They are byte-for-byte the same numbers: the acceptance criterion
//! for 0054 is that re-expressing the bundled components as definitions moves no geometry at all.

use quill_components_ttrpg::{
    ComponentDef, ComponentLibrary, DefColor, DefStroke, PanelDef, RuleDef, SectionDef,
    SectionShape, SplitDef, SplitGranularity, ZebraDef, COMPONENT_DEF_VERSION,
    STATBLOCK_FIELD_ACTIONS, STATBLOCK_FIELD_ATTRIBUTES, STATBLOCK_FIELD_DETAILS,
    STATBLOCK_FIELD_NAME, STATBLOCK_FIELD_OVERVIEW, STATBLOCK_FIELD_REACTIONS, TABLE_FIELD_COLUMNS,
    TABLE_FIELD_HEADER, TABLE_FIELD_ROWS, TABLE_FIELD_ZEBRA,
};

use crate::{
    STATBLOCK_ATTR_STYLE, STATBLOCK_BODY_STYLE, STATBLOCK_TITLE_STYLE, TABLE_CELL_STYLE,
    TABLE_HEADER_STYLE,
};

/// The name the bundled stat-block definition is registered and resolved under.
pub const STATBLOCK_COMPONENT: &str = "statblock";
/// The name the bundled table definition is registered and resolved under.
pub const TABLE_COMPONENT: &str = "table";

/// Padding between a stat block's panel edge and its text, on all four sides.
pub const STATBLOCK_PADDING_PT: f32 = 6.0;

/// The panel's background tint. 8% black — enough to read as a panel on paper, far enough inside
/// the ink limit that it can never be the thing that fails preflight.
const STATBLOCK_FILL: DefColor = DefColor::Gray { v: 0.92 };

/// The panel's outer rule, and the ink its section hairlines are drawn in.
const STATBLOCK_RULE_INK: DefColor = DefColor::Gray { v: 0.35 };
const STATBLOCK_STROKE_PT: f32 = 0.75;

/// Thickness of the hairlines that separate a stat block's sections, and the space above and below
/// each. The thickness deliberately does not advance the cursor — see [`RuleDef`].
const SECTION_RULE_PT: f32 = 0.5;
const SECTION_RULE_GAP_PT: f32 = 2.5;

/// Padding inside each table cell, so text never touches a rule or its neighbour's column.
pub const TABLE_CELL_PADDING_PT: f32 = 3.0;

/// The shade behind alternate rows. Light enough that 9 pt text stays legible over it, and far
/// enough inside the ink limit that it can never be what fails preflight.
const TABLE_ZEBRA_FILL: DefColor = DefColor::Gray { v: 0.94 };

/// The rule under a table's header row.
const TABLE_HEADER_RULE_PT: f32 = 0.75;

/// The fewest *sections* a stat-block fragment may contain (spec 0046).
///
/// One, not two, and the difference matters. A section is coarse — a whole attributes list, a whole
/// actions list — so demanding two per fragment can make the smallest legal cut larger than a
/// frame, at which point nothing is cut and the panel runs off the bottom of the page.
const MIN_SECTIONS_PER_FRAGMENT: usize = 1;

/// The fewest *rows* a table fragment may contain (spec 0045). Two, for the same reason as lines:
/// one row alone under a repeated header reads as a mistake.
const MIN_ROWS_PER_FRAGMENT: usize = 2;

fn section_rule() -> RuleDef {
    RuleDef {
        color: STATBLOCK_RULE_INK,
        thickness_pt: SECTION_RULE_PT,
        gap_above_pt: SECTION_RULE_GAP_PT,
        gap_below_pt: SECTION_RULE_GAP_PT,
        full_width: false,
    }
}

fn lines_section(source: &str) -> SectionDef {
    SectionDef {
        source: source.into(),
        style: STATBLOCK_BODY_STYLE.into(),
        shape: SectionShape::Lines,
        rule_above: Some(section_rule()),
        rule_below: None,
        repeat: false,
    }
}

/// The stat block, as a definition.
///
/// The section order is the one `StatBlock`'s own doc comment states — name, overview, attributes,
/// details, actions, reactions — which is the order the compact layout this mirrors reads in.
/// Getting it wrong puts the creature's type *after* its armour class, which is visibly not a stat
/// block; the first draft of spec 0038 did exactly that and only the render showed it.
pub fn statblock_definition() -> ComponentDef {
    ComponentDef {
        version: COMPONENT_DEF_VERSION,
        name: STATBLOCK_COMPONENT.into(),
        panel: PanelDef {
            fill: Some(STATBLOCK_FILL),
            stroke: Some(DefStroke {
                color: STATBLOCK_RULE_INK,
                width_pt: STATBLOCK_STROKE_PT,
            }),
            padding_pt: STATBLOCK_PADDING_PT,
        },
        sections: vec![
            SectionDef {
                source: STATBLOCK_FIELD_NAME.into(),
                style: STATBLOCK_TITLE_STYLE.into(),
                shape: SectionShape::Text,
                rule_above: None,
                rule_below: None,
                repeat: false,
            },
            lines_section(STATBLOCK_FIELD_OVERVIEW),
            SectionDef {
                source: STATBLOCK_FIELD_ATTRIBUTES.into(),
                style: STATBLOCK_ATTR_STYLE.into(),
                // A colon, not the two spaces spec 0038 first used: `break_by_width` normalizes
                // every run of inter-word whitespace to a single U+0020, so the intended visual gap
                // collapsed and "Armour Class 15 (leather, shield)" read as one sentence. With no
                // bold weight available, punctuation is what distinguishes the key.
                shape: SectionShape::Pairs {
                    separator: ":\u{a0}".into(),
                },
                rule_above: Some(section_rule()),
                rule_below: None,
                repeat: false,
            },
            lines_section(STATBLOCK_FIELD_DETAILS),
            lines_section(STATBLOCK_FIELD_ACTIONS),
            lines_section(STATBLOCK_FIELD_REACTIONS),
        ],
        split: SplitDef {
            // A cut may fall between sections and nowhere else, so an attributes list is never
            // separated from itself and a prose section never breaks mid-run (spec 0046).
            granularity: SplitGranularity::Sections,
            min_items: MIN_SECTIONS_PER_FRAGMENT,
            keep_together: true,
        },
    }
}

/// The table, as a definition.
///
/// No panel of its own — a table is rules and bands, not a box — so `padding_pt` is zero and the
/// inset that keeps text off a rule lives on the cells instead.
pub fn table_definition() -> ComponentDef {
    let rows_shape = |zebra: Option<ZebraDef>| SectionShape::Rows {
        widths: Some(TABLE_FIELD_COLUMNS.into()),
        cell_padding_pt: TABLE_CELL_PADDING_PT,
        zebra,
    };
    ComponentDef {
        version: COMPONENT_DEF_VERSION,
        name: TABLE_COMPONENT.into(),
        panel: PanelDef::default(),
        sections: vec![
            SectionDef {
                source: TABLE_FIELD_HEADER.into(),
                style: TABLE_HEADER_STYLE.into(),
                shape: rows_shape(None),
                rule_above: None,
                rule_below: Some(RuleDef {
                    color: DefColor::Gray { v: 0.35 },
                    thickness_pt: TABLE_HEADER_RULE_PT,
                    gap_above_pt: 0.0,
                    // The header rule is the one that advances by its own thickness, which is why
                    // `RuleDef` expresses that as a gap rather than assuming it.
                    gap_below_pt: TABLE_HEADER_RULE_PT,
                    full_width: true,
                }),
                // The header and its rule are the prefix every continuation re-states (spec 0045).
                repeat: true,
            },
            SectionDef {
                source: TABLE_FIELD_ROWS.into(),
                style: TABLE_CELL_STYLE.into(),
                shape: rows_shape(Some(ZebraDef {
                    fill: TABLE_ZEBRA_FILL,
                    parity: 1,
                    enabled_by: Some(TABLE_FIELD_ZEBRA.into()),
                })),
                rule_above: None,
                rule_below: None,
                repeat: false,
            },
        ],
        split: SplitDef {
            granularity: SplitGranularity::Elements,
            min_items: MIN_ROWS_PER_FRAGMENT,
            keep_together: true,
        },
    }
}

/// The definitions every document has, before any pack adds to them.
pub fn builtin_components() -> ComponentLibrary {
    [statblock_definition(), table_definition()]
        .into_iter()
        .map(|d| (d.name.clone(), d))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_definitions_validate() {
        for def in builtin_components().values() {
            def.validate()
                .unwrap_or_else(|e| panic!("bundled definition is malformed: {e}"));
        }
    }

    #[test]
    fn the_bundled_definitions_resolve_styles_the_default_stylesheet_supplies() {
        let styles = crate::StyleSheet::default();
        for def in builtin_components().values() {
            for name in def.style_names() {
                assert!(
                    styles.paragraph.contains_key(name),
                    "`{}` names style `{name}`, which the default stylesheet does not supply",
                    def.name
                );
            }
        }
    }
}
