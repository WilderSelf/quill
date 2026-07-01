//! TTRPG-native content components (stat blocks, random tables) as portable, first-class
//! objects — addressing the Homebrewery/GM Binder fragmentation where the same content needs
//! different markup per tool.

use serde::{Deserialize, Serialize};

/// A creature/NPC stat block. Sections mirror the common compact layout
/// (Overview / Attributes / Details / Actions / Reactions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
