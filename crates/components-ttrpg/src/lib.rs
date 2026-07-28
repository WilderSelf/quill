//! TTRPG-native content components (stat blocks, random tables) as portable, first-class
//! objects — addressing the Homebrewery/GM Binder fragmentation where the same content needs
//! different markup per tool.

use serde::{Deserialize, Serialize};

/// A creature/NPC stat block. Sections mirror the common compact layout
/// (Overview / Attributes / Details / Actions / Reactions).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StatBlock {
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

/// One row of a random table, covering an inclusive die-roll range `low..=high`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableEntry {
    pub low: u32,
    pub high: u32,
    pub result: String,
}

/// A random table rolled on `die` (e.g. `die = 100` for a d100 table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RandomTable {
    pub die: u32,
    pub entries: Vec<TableEntry>,
}

impl RandomTable {
    /// The result for a given roll, if any entry covers it.
    pub fn lookup(&self, roll: u32) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| roll >= e.low && roll <= e.high)
            .map(|e| e.result.as_str())
    }

    /// Whether the entries cover every value in `1..=die` exactly once (no gaps or overlaps).
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
        expected == self.die + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d6_table() -> RandomTable {
        RandomTable {
            die: 6,
            entries: vec![
                TableEntry {
                    low: 1,
                    high: 3,
                    result: "Goblins".into(),
                },
                TableEntry {
                    low: 4,
                    high: 6,
                    result: "Bandits".into(),
                },
            ],
        }
    }

    #[test]
    fn lookup_finds_the_covering_entry() {
        let t = d6_table();
        assert_eq!(t.lookup(2), Some("Goblins"));
        assert_eq!(t.lookup(5), Some("Bandits"));
        assert_eq!(t.lookup(7), None);
    }

    #[test]
    fn completeness_detects_gaps() {
        assert!(d6_table().is_complete());
        let gappy = RandomTable {
            die: 6,
            entries: vec![TableEntry {
                low: 1,
                high: 3,
                result: "x".into(),
            }],
        };
        assert!(!gappy.is_complete());
    }
}

/// A table: column widths, an optional repeating header, and rows of cells.
///
/// General rather than random-table-specific. A random table is the special case where column 0
/// holds a die range — [`Table::from_random`] builds one — but a rulebook is full of ordinary
/// tables (equipment, prices, encounter difficulty) and they all want the same layout.
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
        let usable: Vec<f32> = self
            .columns
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

    /// Render a [`RandomTable`] as a two-column table of die range and result.
    pub fn from_random(table: &RandomTable) -> Table {
        Table {
            columns: vec![0.2, 0.8],
            header: Some(vec![format!("d{}", table.die), "Result".into()]),
            rows: table
                .entries
                .iter()
                .map(|e| vec![format_range(e.low, e.high), e.result.clone()])
                .collect(),
            zebra: true,
        }
    }
}

/// A die range as a reader would write it: `7` for a single value, `11-25` for a span.
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
    fn a_random_table_renders_as_range_and_result() {
        let t = RandomTable {
            die: 100,
            entries: vec![
                TableEntry {
                    low: 1,
                    high: 10,
                    result: "Nothing".into(),
                },
                TableEntry {
                    low: 11,
                    high: 11,
                    result: "A single".into(),
                },
            ],
        };
        let table = Table::from_random(&t);
        assert_eq!(
            table.header.as_deref(),
            Some(&["d100".to_string(), "Result".to_string()][..])
        );
        assert_eq!(table.rows[0][0], "1-10");
        assert_eq!(
            table.rows[1][0], "11",
            "a one-value range must not read `11-11`"
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
