//! Component *definitions* — a component as data rather than as a Rust type (spec 0054).
//!
//! [`StatBlock`](crate::StatBlock) and [`Table`](crate::Table) are two shapes a TTRPG publisher
//! wants; there are hundreds. A `ComponentDef` declares what `layout-engine`'s two hand-written
//! measurement functions used to hard-code — an ordered list of styled sections, a panel, and the
//! rules a section list needs to be cut across a frame boundary — so a publisher can express a
//! third shape without a Rust compiler, and so a content pack (spec 0055) has something to carry.
//!
//! Nothing here emits geometry. A definition supplies *inputs*; `layout-engine` interprets them
//! through the same path body text goes through, which is what keeps spec 0050's preflight
//! authoritative over a component that arrived from a stranger. See `docs/roadmap.md`, "The
//! decision this milestone turns on".

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The declaration format this build understands.
///
/// Its own integer, not `FORMAT_VERSION`, on spec 0053's reasoning: a definition is not a document,
/// and a pack full of definitions should not need re-versioning because the document model grew a
/// field definitions never see. A bump is owed when the *shape declared here* changes meaning.
pub const COMPONENT_DEF_VERSION: u32 = 1;

/// A colour, in the same three families [`quill_core_model::Color`] uses.
///
/// Duplicated here rather than depended on, because `core-model` depends on *this* crate — a
/// component definition is a portable object that must be expressible without a document. The
/// layout engine converts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "snake_case")]
pub enum DefColor {
    /// Additive grey, `0.0` black … `1.0` white — matching `Color::Gray`.
    Gray {
        v: f32,
    },
    Cmyk {
        c: f32,
        m: f32,
        y: f32,
        k: f32,
    },
}

/// A rule's outline width and ink.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DefStroke {
    pub color: DefColor,
    pub width_pt: f32,
}

/// The decorated box a component's sections sit inside.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PanelDef {
    /// Background tint. `None` for a component with no panel of its own — a table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<DefColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<DefStroke>,
    /// Inset on all four sides, between the panel edge and its content.
    ///
    /// One number rather than four: a padding an author could set asymmetrically is a padding an
    /// author can set to zero on the side where text would then sit on the rule.
    #[serde(default)]
    pub padding_pt: f32,
}

/// A hairline drawn between sections.
///
/// `gap_above_pt` and `gap_below_pt` are what advance the vertical cursor; **the rule's own
/// thickness does not**. That is deliberate rather than an oversight inherited from the code this
/// generalizes: it lets one shape express both bundled rules — a stat block's hairline sits inside
/// its lower gap, and a table's header rule advances by exactly its thickness because its
/// `gap_below_pt` is set to that thickness.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RuleDef {
    pub color: DefColor,
    pub thickness_pt: f32,
    #[serde(default)]
    pub gap_above_pt: f32,
    #[serde(default)]
    pub gap_below_pt: f32,
    /// Span the panel's full width rather than its padded measure. A table's header rule runs edge
    /// to edge; a stat block's section rule is inset with the text it separates.
    #[serde(default)]
    pub full_width: bool,
}

/// Shading behind alternate elements of a `rows` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZebraDef {
    pub fill: DefColor,
    /// Which elements are banded: `1` shades the second, fourth, … row, which is the convention —
    /// banding the *first* row makes a header-less table look like it has one.
    #[serde(default = "one")]
    pub parity: usize,
    /// A boolean instance field that switches banding off. `None` means always on.
    ///
    /// Banding is per-*instance* in the shipped table (`Table::zebra`) and per-*definition*
    /// everywhere else, so it is expressed as a definition-declared default with an instance
    /// override rather than as one or the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<String>,
}

fn one() -> usize {
    1
}

/// How a section's field value becomes runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum SectionShape {
    /// One run, always — even when the field is missing or empty. The component's title.
    Text,
    /// One run per line, and *nothing at all* when the list is empty: an absent section must not
    /// leave its rule behind.
    Lines,
    /// One run per pair, set `key⟨separator⟩value`.
    ///
    /// The key's own words are joined by U+00A0 so `Armour Class:` cannot break across lines
    /// (spec 0048) — a key/value pairing split over a line break stops reading as one.
    Pairs {
        #[serde(default = "colon_space")]
        separator: String,
    },
    /// One row of cells per element, laid across the component's columns.
    Rows {
        /// Instance field holding the column width fractions. Absent, degenerate or the wrong
        /// length falls back to an equal split (`Table::normalized_columns`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        widths: Option<String>,
        /// Inset inside each cell, taken off the cell's *measure* so a wrapped cell stays inside
        /// its column rather than being broken wide and then drawn inset.
        #[serde(default)]
        cell_padding_pt: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zebra: Option<ZebraDef>,
    },
}

fn colon_space() -> String {
    ":\u{a0}".to_string()
}

/// One styled section of a component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionDef {
    /// The instance field this section renders.
    pub source: String,
    /// Paragraph style name, resolved against the document stylesheet. One that does not resolve
    /// falls back to the default style rather than failing — the authoring posture.
    pub style: String,
    #[serde(flatten)]
    pub shape: SectionShape,
    /// A hairline above this section. Never drawn for the first section that actually emits: it has
    /// the panel's own edge above it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_above: Option<RuleDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_below: Option<RuleDef>,
    /// Re-stated at the top of every continuation when the component is cut (spec 0045) — a table's
    /// header row.
    #[serde(default)]
    pub repeat: bool,
}

/// What a component's fragments are made of when it is cut across a frame boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitGranularity {
    /// One item per emitted section — a stat block, whose attributes list is never separated from
    /// itself.
    #[default]
    Sections,
    /// One item per element of the last non-repeated section — a table's rows.
    Elements,
    /// Indivisible: the component moves whole or not at all.
    Whole,
}

/// How a component may be cut across a frame boundary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SplitDef {
    #[serde(default)]
    pub granularity: SplitGranularity,
    /// The fewest items a fragment may contain. One for coarse items (a whole section), two for
    /// fine ones (a row) — a lone row under a repeated header reads as a mistake, while demanding
    /// two *sections* can make the smallest legal cut larger than a frame.
    #[serde(default = "one")]
    pub min_items: usize,
    /// Prefer moving whole to the next frame over cutting, when the component would fit there
    /// entire (spec 0046).
    #[serde(default = "yes")]
    pub keep_together: bool,
}

fn yes() -> bool {
    true
}

impl Default for SplitDef {
    fn default() -> Self {
        Self {
            granularity: SplitGranularity::default(),
            min_items: 1,
            keep_together: true,
        }
    }
}

/// A component, declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentDef {
    /// The declaration format version. A value above [`COMPONENT_DEF_VERSION`] is refused, not
    /// best-effort loaded: a definition this build half-understands lays out geometry that goes to
    /// a printer.
    #[serde(default = "current_version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub panel: PanelDef,
    pub sections: Vec<SectionDef>,
    #[serde(default)]
    pub split: SplitDef,
}

fn current_version() -> u32 {
    COMPONENT_DEF_VERSION
}

/// Everything a component definition can be wrong about.
///
/// Every variant names the definition, because an error that does not name the thing that is wrong
/// sends the reader to the wrong file — spec 0025's posture, applied to content from a stranger.
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentDefError {
    UnsupportedVersion {
        def: String,
        found: u32,
        supported: u32,
    },
    EmptyName,
    NoSections {
        def: String,
    },
    SectionMissingSource {
        def: String,
        index: usize,
    },
    SectionMissingStyle {
        def: String,
        index: usize,
    },
    DuplicateSectionSource {
        def: String,
        source: String,
    },
    /// A definition filed under one name that calls itself another. `Block::Component { def }`
    /// resolves the key, so a mismatch means the definition that was validated is not the one that
    /// would be laid out.
    NameMismatch {
        key: String,
        def: String,
    },
}

impl fmt::Display for ComponentDefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComponentDefError::UnsupportedVersion {
                def,
                found,
                supported,
            } => write!(
                f,
                "component definition `{def}` declares version {found}, newer than this build \
                 supports (up to {supported}); upgrade Quill to use it"
            ),
            ComponentDefError::EmptyName => {
                write!(f, "a component definition has an empty `name`")
            }
            ComponentDefError::NoSections { def } => {
                write!(f, "component definition `{def}` declares no sections")
            }
            ComponentDefError::SectionMissingSource { def, index } => write!(
                f,
                "component definition `{def}`, section {index}: `source` is empty"
            ),
            ComponentDefError::SectionMissingStyle { def, index } => write!(
                f,
                "component definition `{def}`, section {index}: `style` is empty"
            ),
            ComponentDefError::DuplicateSectionSource { def, source } => write!(
                f,
                "component definition `{def}` renders field `{source}` in two sections, which \
                 would set it twice"
            ),
            ComponentDefError::NameMismatch { key, def } => write!(
                f,
                "component definition filed under `{key}` calls itself `{def}`"
            ),
        }
    }
}

impl std::error::Error for ComponentDefError {}

impl ComponentDef {
    /// Refuse a definition this build cannot lay out correctly.
    pub fn validate(&self) -> Result<(), ComponentDefError> {
        if self.name.trim().is_empty() {
            return Err(ComponentDefError::EmptyName);
        }
        if self.version > COMPONENT_DEF_VERSION {
            return Err(ComponentDefError::UnsupportedVersion {
                def: self.name.clone(),
                found: self.version,
                supported: COMPONENT_DEF_VERSION,
            });
        }
        if self.sections.is_empty() {
            return Err(ComponentDefError::NoSections {
                def: self.name.clone(),
            });
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.sections.len());
        for (index, s) in self.sections.iter().enumerate() {
            if s.source.trim().is_empty() {
                return Err(ComponentDefError::SectionMissingSource {
                    def: self.name.clone(),
                    index,
                });
            }
            if s.style.trim().is_empty() {
                return Err(ComponentDefError::SectionMissingStyle {
                    def: self.name.clone(),
                    index,
                });
            }
            if seen.contains(&s.source.as_str()) {
                return Err(ComponentDefError::DuplicateSectionSource {
                    def: self.name.clone(),
                    source: s.source.clone(),
                });
            }
            seen.push(&s.source);
        }
        Ok(())
    }

    /// The names of the paragraph styles this definition resolves, in section order.
    ///
    /// Used by pack extraction (spec 0057) to carry a definition's styles with it: a definition
    /// whose styles were left behind produces a component set in the default face.
    pub fn style_names(&self) -> Vec<&str> {
        self.sections.iter().map(|s| s.style.as_str()).collect()
    }
}

/// One authored field of a component instance.
///
/// A closed set rather than free-form JSON: the interpreter has to know how to turn a value into
/// runs, and a value it cannot is a value it would have to guess about.
///
/// **Adjacently** tagged, not internally tagged. An internal tag cannot represent a newtype variant
/// whose payload is a sequence — `Lines`, `Rows` and `Widths` are all sequences — and `serde`
/// reports that only at *runtime*, as "invalid type: map, expected a sequence", from whatever
/// happens to deserialize a manifest first. The round-trip test below is what pins it down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FieldValue {
    Text(String),
    Lines(Vec<String>),
    Pairs(Vec<(String, String)>),
    Rows(Vec<Vec<String>>),
    /// Column width fractions. Normalized on use, so `[1, 3]` and `[0.25, 0.75]` mean the same.
    Widths(Vec<f32>),
    Flag(bool),
}

impl FieldValue {
    /// The value as text, for a `text` section. Anything else reads as empty rather than as a
    /// debug-formatted struct: authoring-side, a mistyped field costs the *look*, never a panic.
    pub fn as_text(&self) -> &str {
        match self {
            FieldValue::Text(t) => t,
            _ => "",
        }
    }

    pub fn as_lines(&self) -> &[String] {
        match self {
            FieldValue::Lines(l) => l,
            _ => &[],
        }
    }

    pub fn as_pairs(&self) -> &[(String, String)] {
        match self {
            FieldValue::Pairs(p) => p,
            _ => &[],
        }
    }

    pub fn as_rows(&self) -> &[Vec<String>] {
        match self {
            FieldValue::Rows(r) => r,
            _ => &[],
        }
    }

    pub fn as_widths(&self) -> &[f32] {
        match self {
            FieldValue::Widths(w) => w,
            _ => &[],
        }
    }

    /// A flag's value, or `true` for a field that is missing or of another kind — so a `zebra`
    /// switch an author never wrote keeps the definition's default.
    pub fn as_flag(&self) -> bool {
        match self {
            FieldValue::Flag(b) => *b,
            _ => true,
        }
    }
}

/// The authored content of one component instance: field name to value.
///
/// A `BTreeMap` rather than a `HashMap` so the serialized manifest is stable and git-diffable —
/// the same reasoning `StyleSheet` uses.
pub type ComponentFields = BTreeMap<String, FieldValue>;

/// The definitions available to a document: bundled ones, plus whatever its packs supply.
pub type ComponentLibrary = BTreeMap<String, ComponentDef>;

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_section() -> SectionDef {
        SectionDef {
            source: "name".into(),
            style: "body".into(),
            shape: SectionShape::Text,
            rule_above: None,
            rule_below: None,
            repeat: false,
        }
    }

    fn ok_def() -> ComponentDef {
        ComponentDef {
            version: COMPONENT_DEF_VERSION,
            name: "thing".into(),
            panel: PanelDef::default(),
            sections: vec![ok_section()],
            split: SplitDef::default(),
        }
    }

    #[test]
    fn a_well_formed_definition_validates() {
        assert_eq!(ok_def().validate(), Ok(()));
    }

    #[test]
    fn a_newer_version_is_refused_by_name() {
        let mut d = ok_def();
        d.version = COMPONENT_DEF_VERSION + 1;
        let err = d.validate().unwrap_err();
        assert!(
            err.to_string().contains("thing"),
            "the error must name the definition: {err}"
        );
        assert!(matches!(err, ComponentDefError::UnsupportedVersion { .. }));
    }

    #[test]
    fn malformed_shapes_are_each_their_own_error() {
        let mut empty_name = ok_def();
        empty_name.name = "  ".into();
        assert_eq!(empty_name.validate(), Err(ComponentDefError::EmptyName));

        let mut no_sections = ok_def();
        no_sections.sections.clear();
        assert!(matches!(
            no_sections.validate(),
            Err(ComponentDefError::NoSections { .. })
        ));

        let mut no_source = ok_def();
        no_source.sections[0].source = String::new();
        assert!(matches!(
            no_source.validate(),
            Err(ComponentDefError::SectionMissingSource { index: 0, .. })
        ));

        let mut no_style = ok_def();
        no_style.sections[0].style = String::new();
        assert!(matches!(
            no_style.validate(),
            Err(ComponentDefError::SectionMissingStyle { index: 0, .. })
        ));

        let mut dup = ok_def();
        dup.sections.push(ok_section());
        assert!(matches!(
            dup.validate(),
            Err(ComponentDefError::DuplicateSectionSource { .. })
        ));
    }

    #[test]
    fn a_definition_round_trips_through_json() {
        let d = ComponentDef {
            sections: vec![
                ok_section(),
                SectionDef {
                    source: "rows".into(),
                    style: "table-cell".into(),
                    shape: SectionShape::Rows {
                        widths: Some("columns".into()),
                        cell_padding_pt: 3.0,
                        zebra: Some(ZebraDef {
                            fill: DefColor::Gray { v: 0.94 },
                            parity: 1,
                            enabled_by: Some("zebra".into()),
                        }),
                    },
                    rule_above: None,
                    rule_below: None,
                    repeat: false,
                },
            ],
            ..ok_def()
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: ComponentDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    /// The strongest form of spec 0054's "a pack may not make output less press-correct": a
    /// definition *cannot express* RGB decoration at all, so there is no path by which a component
    /// from a stranger introduces a colour space PDF/X-1a forbids.
    #[test]
    fn a_definition_cannot_declare_rgb_decoration() {
        assert!(
            serde_json::from_str::<DefColor>(r#"{"space":"rgb","r":1.0,"g":0.0,"b":0.0}"#).is_err()
        );
        for ok in [
            r#"{"space":"gray","v":0.5}"#,
            r#"{"space":"cmyk","c":0.1,"m":0.2,"y":0.3,"k":0.4}"#,
        ] {
            serde_json::from_str::<DefColor>(ok).expect("the two press-safe families parse");
        }
    }

    /// Every variant, through JSON and back. Not a formality: the sequence-carrying variants do
    /// not survive an internally tagged representation, and nothing else in the crate would have
    /// noticed — the failure surfaces as a manifest that will not load.
    #[test]
    fn every_field_value_round_trips_through_json() {
        let values = vec![
            FieldValue::Text("a title".into()),
            FieldValue::Lines(vec!["one".into(), "two".into()]),
            FieldValue::Pairs(vec![("Armour Class".into(), "15".into())]),
            FieldValue::Rows(vec![vec!["a".into(), "b".into()], vec!["c".into()]]),
            FieldValue::Widths(vec![0.25, 0.75]),
            FieldValue::Flag(false),
        ];
        for v in values {
            let json = serde_json::to_string(&v).expect("serialize");
            let back: FieldValue = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{v:?} did not survive {json}: {e}"));
            assert_eq!(back, v);
        }

        // And the whole map, which is what a manifest actually carries.
        let mut fields = ComponentFields::new();
        fields.insert("title".into(), FieldValue::Text("x".into()));
        fields.insert("rows".into(), FieldValue::Rows(vec![vec!["y".into()]]));
        let json = serde_json::to_string(&fields).expect("serialize");
        let back: ComponentFields = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, fields);
    }

    #[test]
    fn a_field_of_the_wrong_kind_reads_as_empty_rather_than_panicking() {
        let v = FieldValue::Text("x".into());
        assert_eq!(v.as_text(), "x");
        assert!(v.as_lines().is_empty());
        assert!(v.as_rows().is_empty());
        assert!(v.as_pairs().is_empty());
        assert!(v.as_widths().is_empty());
        // A missing switch keeps the definition's default rather than turning it off.
        assert!(v.as_flag());
        assert!(!FieldValue::Flag(false).as_flag());
    }
}
