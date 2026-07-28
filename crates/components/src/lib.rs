//! Portable content components — panels, tables and range tables — as first-class objects that
//! exist independently of any document, so the same record can be authored once and set by any
//! tool that understands the format.
//!
//! Deliberately domain-neutral (spec 0062). A panel is a titled, bordered record of named
//! sections: a creature, a recipe, a specimen, a part number. A range table partitions `1..=max`:
//! a d100 roll, a severity scale, a postage band. The genre lives in the *content* — which is what
//! spec 0054's declared components made expressible — and a mechanism only one genre can use is a
//! defect here.

use serde::{Deserialize, Serialize};

mod def;

pub use def::{
    ComponentDef, ComponentDefError, ComponentFields, ComponentLibrary, DefColor, DefStroke,
    FieldValue, PanelDef, RuleDef, SectionDef, SectionShape, SplitDef, SplitGranularity, ZebraDef,
    COMPONENT_DEF_VERSION,
};

/// Field names the bundled `statblock` definition renders (spec 0054). Constants rather than
/// literals because the definition and the conversion have to agree, and a typo in either produces
/// a silently empty section rather than an error.
pub const STATBLOCK_FIELD_NAME: &str = "name";
pub const STATBLOCK_FIELD_OVERVIEW: &str = "overview";
pub const STATBLOCK_FIELD_ATTRIBUTES: &str = "attributes";
pub const STATBLOCK_FIELD_DETAILS: &str = "details";
pub const STATBLOCK_FIELD_ACTIONS: &str = "actions";
pub const STATBLOCK_FIELD_REACTIONS: &str = "reactions";

/// Field names the bundled `table` definition renders.
pub const TABLE_FIELD_HEADER: &str = "header";
pub const TABLE_FIELD_ROWS: &str = "rows";
pub const TABLE_FIELD_COLUMNS: &str = "columns";
pub const TABLE_FIELD_ZEBRA: &str = "zebra";

/// Column widths normalized to sum to 1, or an equal split when they cannot be.
///
/// A zero, negative or non-finite width is not an error here — this is authoring-side, and the
/// posture the rest of the model takes is that bad input costs the *look*, never the content. An
/// equal split is wrong-looking and obvious; a degenerate column would silently swallow cells.
pub fn normalized_widths(widths: &[f32], count: usize) -> Vec<f32> {
    let usable: Vec<f32> = widths
        .iter()
        .copied()
        .filter(|w| w.is_finite() && *w > 0.0)
        .collect();
    let total: f32 = usable.iter().sum();
    if usable.len() != count || total <= 0.0 {
        return vec![1.0 / count.max(1) as f32; count];
    }
    usable.iter().map(|w| w / total).collect()
}

/// A titled record of named sections, set as a bordered panel.
///
/// The section names are fixed. This type is the *bundled specialization* — spec 0054's
/// [`ComponentDef`](crate::ComponentDef) is the general mechanism, and a publisher whose record has
/// different sections declares one rather than reaching for this.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub name: String,
    #[serde(default)]
    pub overview: Vec<String>,
    #[serde(default)]
    pub attributes: Vec<(String, String)>,
    #[serde(default)]
    pub details: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub reactions: Vec<String>,
}

impl Panel {
    /// This stat block as component instance fields (spec 0054). See [`Table::to_fields`].
    pub fn to_fields(&self) -> ComponentFields {
        let mut fields = ComponentFields::new();
        fields.insert(
            STATBLOCK_FIELD_NAME.into(),
            FieldValue::Text(self.name.clone()),
        );
        for (field, lines) in [
            (STATBLOCK_FIELD_OVERVIEW, &self.overview),
            (STATBLOCK_FIELD_DETAILS, &self.details),
            (STATBLOCK_FIELD_ACTIONS, &self.actions),
            (STATBLOCK_FIELD_REACTIONS, &self.reactions),
        ] {
            fields.insert(field.into(), FieldValue::Lines(lines.clone()));
        }
        fields.insert(
            STATBLOCK_FIELD_ATTRIBUTES.into(),
            FieldValue::Pairs(self.attributes.clone()),
        );
        fields
    }
}

/// One row of a range table, covering the inclusive range `low..=high`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeEntry {
    pub low: u32,
    pub high: u32,
    pub value: String,
}

/// A table whose entries partition `1..=max` — a lookup by number rather than by key.
///
/// A d100 roll table is the case this was built for, but so is a severity scale, a postage band and
/// a damage-deposit schedule; `max` is the upper bound of the partition and carries no claim about
/// where the number came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeTable {
    pub max: u32,
    /// The heading column 0 is given when this is set as a [`Table`]. Empty falls back to
    /// `1–{max}`; `d100` is what a roll table would put here, which is how the genre stays in the
    /// content rather than in the type.
    #[serde(default)]
    pub label: String,
    pub entries: Vec<RangeEntry>,
}

impl RangeTable {
    /// The value for a given number, if any entry covers it.
    pub fn lookup(&self, n: u32) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| n >= e.low && n <= e.high)
            .map(|e| e.value.as_str())
    }

    /// Whether the entries cover every value in `1..=max` exactly once (no gaps or overlaps).
    pub fn is_complete(&self) -> bool {
        let mut sorted = self.entries.clone();
        sorted.sort_by_key(|e| e.low);
        let mut expected = 1;
        for e in &sorted {
            if e.low != expected || e.high < e.low {
                return false;
            }
            expected = e.high + 1;
        }
        expected == self.max + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn six_way_table() -> RangeTable {
        RangeTable {
            max: 6,
            label: String::new(),
            entries: vec![
                RangeEntry {
                    low: 1,
                    high: 3,
                    value: "Goblins".into(),
                },
                RangeEntry {
                    low: 4,
                    high: 6,
                    value: "Bandits".into(),
                },
            ],
        }
    }

    #[test]
    fn lookup_finds_the_covering_entry() {
        let t = six_way_table();
        assert_eq!(t.lookup(2), Some("Goblins"));
        assert_eq!(t.lookup(5), Some("Bandits"));
        assert_eq!(t.lookup(7), None);
    }

    #[test]
    fn completeness_detects_gaps() {
        assert!(six_way_table().is_complete());
        let gappy = RangeTable {
            max: 6,
            label: String::new(),
            entries: vec![RangeEntry {
                low: 1,
                high: 3,
                value: "x".into(),
            }],
        };
        assert!(!gappy.is_complete());
    }
}

/// A table: column widths, an optional repeating header, and rows of cells.
///
/// General rather than lookup-specific. A [`RangeTable`] is the special case where column 0 holds a
/// numeric range — [`Table::from_range_table`] builds one — but a book is full of ordinary tables
/// (parts, prices, comparisons) and they all want the same layout.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Table {
    /// Column widths as fractions of the available measure. Normalized on use, so `[1, 3]` and
    /// `[0.25, 0.75]` mean the same thing — an author should not have to make them sum to one.
    pub columns: Vec<f32>,
    /// A header row, set apart from the body. Optional: many tables are just rows.
    #[serde(default)]
    pub header: Option<Vec<String>>,
    pub rows: Vec<Vec<String>>,
    /// Shade alternate rows. On by default: a wide table without banding is hard to read across,
    /// and this is exactly the kind of thing a beginner would not think to switch on.
    #[serde(default = "yes")]
    pub zebra: bool,
}

fn yes() -> bool {
    true
}

impl Table {
    /// Column widths normalized to sum to 1, or an equal split when they cannot be.
    ///
    /// A zero, negative or non-finite width is not an error here — this is authoring-side, and the
    /// posture the rest of the model takes is that bad input costs the *look*, never the content.
    /// An equal split is wrong-looking and obvious; a degenerate column would silently swallow
    /// cells.
    pub fn normalized_columns(&self, count: usize) -> Vec<f32> {
        normalized_widths(&self.columns, count)
    }

    /// How many columns the table actually has: the widest row, or the header.
    pub fn column_count(&self) -> usize {
        self.header
            .as_ref()
            .map(|h| h.len())
            .into_iter()
            .chain(self.rows.iter().map(|r| r.len()))
            .max()
            .unwrap_or(0)
    }

    /// This table as component instance fields (spec 0054).
    ///
    /// `Block::Table` keeps its authored shape — a `columns`/`header`/`rows` struct is a better
    /// thing to write by hand than a field map — and converts here, so there is exactly one
    /// measurement path and the bundled table is measured through the same interpreter a packed
    /// component is.
    pub fn to_fields(&self) -> ComponentFields {
        let mut fields = ComponentFields::new();
        if let Some(header) = &self.header {
            fields.insert(
                TABLE_FIELD_HEADER.into(),
                FieldValue::Rows(vec![header.clone()]),
            );
        }
        fields.insert(TABLE_FIELD_ROWS.into(), FieldValue::Rows(self.rows.clone()));
        fields.insert(
            TABLE_FIELD_COLUMNS.into(),
            FieldValue::Widths(self.columns.clone()),
        );
        fields.insert(TABLE_FIELD_ZEBRA.into(), FieldValue::Flag(self.zebra));
        fields
    }

    /// Set a [`RangeTable`] as a two-column table of range and value.
    ///
    /// Column 0's heading comes from the table's own `label`, falling back to `1–{max}`. It was
    /// `d{die}` before spec 0062, which asserted the number was a die roll; a roll table now says
    /// so itself by setting `label` to `d100`.
    pub fn from_range_table(table: &RangeTable) -> Table {
        let heading = if table.label.is_empty() {
            format!("1–{}", table.max)
        } else {
            table.label.clone()
        };
        Table {
            columns: vec![0.2, 0.8],
            header: Some(vec![heading, "Result".into()]),
            rows: table
                .entries
                .iter()
                .map(|e| vec![format_range(e.low, e.high), e.value.clone()])
                .collect(),
            zebra: true,
        }
    }
}

/// A range as a reader would write it: `7` for a single value, `11-25` for a span.
///
/// The singleton case is the one that ships and then embarrasses — `7-7` in a printed book.
fn format_range(low: u32, high: u32) -> String {
    if low == high {
        low.to_string()
    } else {
        format!("{low}-{high}")
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    #[test]
    fn a_range_table_renders_as_range_and_value() {
        let t = RangeTable {
            max: 100,
            label: String::new(),
            entries: vec![
                RangeEntry {
                    low: 1,
                    high: 10,
                    value: "Nothing".into(),
                },
                RangeEntry {
                    low: 11,
                    high: 11,
                    value: "A single".into(),
                },
            ],
        };
        let table = Table::from_range_table(&t);
        assert_eq!(
            table.header.as_deref(),
            Some(&["1\u{2013}100".to_string(), "Result".to_string()][..]),
            "an unlabelled range table heads column 0 with its own bounds, not with a die"
        );
        assert_eq!(table.rows[0][0], "1-10");
        assert_eq!(
            table.rows[1][0], "11",
            "a one-value range must not read `11-11`"
        );

        // The genre is content, not mechanism: a roll table says so by labelling itself.
        let rolled = RangeTable {
            label: "d100".into(),
            ..t
        };
        assert_eq!(
            Table::from_range_table(&rolled).header.as_deref(),
            Some(&["d100".to_string(), "Result".to_string()][..])
        );
    }

    #[test]
    fn column_widths_are_normalized_and_degenerate_input_falls_back() {
        let t = Table {
            columns: vec![1.0, 3.0],
            rows: vec![vec!["a".into(), "b".into()]],
            ..Default::default()
        };
        let w = t.normalized_columns(2);
        assert!((w[0] - 0.25).abs() < 0.001 && (w[1] - 0.75).abs() < 0.001);

        // Wrong count, zero, and negative all fall back to an equal split rather than producing a
        // column that would swallow its cells.
        for columns in [vec![1.0], vec![0.0, 0.0], vec![-1.0, 2.0]] {
            let t = Table {
                columns,
                ..t.clone()
            };
            let w = t.normalized_columns(2);
            assert!(
                (w[0] - 0.5).abs() < 0.001,
                "expected an equal split, got {w:?}"
            );
        }
    }

    #[test]
    fn column_count_follows_the_widest_row() {
        let t = Table {
            rows: vec![vec!["a".into()], vec!["a".into(), "b".into(), "c".into()]],
            ..Default::default()
        };
        assert_eq!(t.column_count(), 3);
        assert_eq!(Table::default().column_count(), 0);
    }
}
