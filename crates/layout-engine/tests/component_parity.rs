//! Spec 0054's acceptance criterion: re-expressing the two bundled components as *definitions*
//! moves no geometry at all.
//!
//! The digests below were re-derived from the **pre-change engine** — checked out at the commit
//! before the interpreter landed, with this exact file compiled against it — and then confirmed
//! against the post-change one. That is the discipline spec 0051 established for its equivalence
//! digest, and it is the only thing standing in front of this increment's real failure mode, which
//! is subtle geometric drift rather than a broken build.
//!
//! An assertion here failing means one of two things. Either the interpreter is wrong, or a
//! *deliberate* change to component geometry has been made — in which case the digest is
//! re-derived by the same procedure and the reason is recorded in the spec. It is never correct to
//! paste in whatever number the test printed.
//!
//! The corpus is chosen to cover what a single simple case would not: wrapped cells, zebra bands,
//! section rules, an absent section, a header-less table, an empty table, and both split paths.

use quill_core_model::Rect;
use quill_core_model::{Asset, Block, BlockId, Color, Panel, StyleSheet, Table};
use quill_layout_engine::{lay_out_in_frame, Frame, LaidOutPage};
use quill_text_layout::{MonospaceRunMetrics, NoHyphenator};

/// Fixed em ratio rather than a shaped font, for the same reason the perf harness uses one: the
/// digest has to be identical on every machine, and a font lookup is not.
const METRICS: MonospaceRunMetrics = MonospaceRunMetrics { em_ratio: 0.6 };

const INK: Color = Color::Gray { v: 0.0 };

/// FNV-1a over the `Debug` rendering of the placed pages.
///
/// `Debug` rather than a hand-written walk over the fields that "matter": the whole point is to
/// catch a field that moved, including one nobody thought to list. A hand-written digest silently
/// stops covering a dimension the moment `PlacedBlock` grows one.
fn digest(pages: &[LaidOutPage]) -> u64 {
    let text = format!("{pages:?}");
    digest_str(&text)
}

fn digest_str(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The digest with spec 0063's two new `Debug` fields textually removed.
///
/// The digest is deliberately `Debug`-based so a new field cannot slip past it — which means a new
/// field also moves it even when no geometry did. Stripping them recovers the pre-0063 rendering
/// exactly, so the old constant still has to match: that is what separates "the struct grew" from
/// "the layout moved", and it is the only reason the constants below could be re-derived.
fn digest_pre_0063(pages: &[LaidOutPage]) -> u64 {
    let text = format!("{pages:?}");
    let text = regex_free_strip(&text, "spans: [", "]");
    let text = regex_free_strip(&text, "run_colors: [", "]");
    digest_str(&text)
}

/// Remove every `open .. close` region, plus the `, ` that separated it from the next field.
fn regex_free_strip(text: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        let after = &rest[i + open.len()..];
        let j = after.find(close).expect("a Debug list closes");
        let mut tail = &after[j + close.len()..];
        tail = tail.strip_prefix(", ").unwrap_or(tail);
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn lay(content: Vec<Block>, w_pt: f32, h_pt: f32) -> Vec<LaidOutPage> {
    let mut content = content;
    for (i, b) in content.iter_mut().enumerate() {
        b.set_id(BlockId(i as u64 + 1));
    }
    let assets: Vec<Asset> = Vec::new();
    lay_out_in_frame(
        &content,
        &assets,
        &StyleSheet::default(),
        &Frame {
            rect: Rect {
                x_pt: 0.0,
                y_pt: 0.0,
                w_pt,
                h_pt,
            },
        },
        &METRICS,
        &NoHyphenator,
    )
}

fn creature() -> Panel {
    Panel {
        name: "Cave Troll".into(),
        overview: vec!["Large giant, chaotic evil.".into()],
        attributes: vec![
            ("Armour Class".into(), "15 (natural armour)".into()),
            ("Hit Points".into(), "84 (8d10 + 40)".into()),
            ("Speed".into(), "30 ft.".into()),
        ],
        details: vec![
            "Regeneration. The troll regains 10 hit points at the start of its turn. If it \
             takes acid or fire damage, this trait does not function at the start of its next \
             turn."
                .into(),
        ],
        actions: vec![
            "Multiattack. The troll makes three attacks: one with its bite and two with its \
             claws."
                .into(),
            "Bite. Melee Weapon Attack: +7 to hit, reach 5 ft., one target.".into(),
        ],
        reactions: vec!["Parry. The troll adds 3 to its AC against one melee attack.".into()],
    }
}

/// A creature with no overview and no reactions: the sections that emit nothing must leave no
/// hairline and no cut boundary behind.
fn sparse_creature() -> Panel {
    Panel {
        overview: Vec::new(),
        reactions: Vec::new(),
        ..creature()
    }
}

fn equipment_table() -> Table {
    Table {
        columns: vec![3.0, 1.0, 2.0],
        header: Some(vec!["Item".into(), "Cost".into(), "Weight".into()]),
        rows: (1..=9)
            .map(|i| {
                vec![
                    format!(
                        "Item number {i}, whose name is deliberately long enough to wrap in a \
                         narrow column and push its whole row down"
                    ),
                    format!("{i} gp"),
                    format!("{i} lb."),
                ]
            })
            .collect(),
        zebra: true,
    }
}

fn headerless_table() -> Table {
    Table {
        columns: vec![1.0, 1.0],
        header: None,
        rows: vec![
            vec!["Dagger".into(), "1d4".into()],
            vec!["Shortsword".into(), "1d6".into()],
            vec!["Greataxe".into(), "1d12".into()],
        ],
        zebra: false,
    }
}

/// Every case, and the frame it is laid into. A short frame is what sends a block down the split
/// path; a tall one keeps it whole.
fn corpus() -> Vec<(&'static str, Vec<Block>, f32, f32)> {
    let sb = |panel: Panel| Block::Panel {
        id: BlockId::UNASSIGNED,
        panel,
        color: INK,
    };
    let tb = |table: Table| Block::Table {
        id: BlockId::UNASSIGNED,
        table,
        color: INK,
    };
    vec![
        ("statblock whole", vec![sb(creature())], 300.0, 700.0),
        ("statblock narrow", vec![sb(creature())], 150.0, 700.0),
        (
            "statblock absent sections",
            vec![sb(sparse_creature())],
            300.0,
            700.0,
        ),
        // Short enough that the panel cannot fit whole and must cut at a section boundary
        // (spec 0046).
        ("statblock split", vec![sb(creature())], 300.0, 120.0),
        ("table whole", vec![tb(equipment_table())], 360.0, 700.0),
        // Wrapped cells in a narrow measure: a wrapped cell must push its row down.
        ("table wrapped", vec![tb(equipment_table())], 180.0, 700.0),
        // Short enough that rows carry over and the header is re-stated (spec 0045).
        ("table split", vec![tb(equipment_table())], 360.0, 150.0),
        (
            "table headerless",
            vec![tb(headerless_table())],
            360.0,
            700.0,
        ),
        (
            "table empty",
            vec![tb(Table::default()), Block::body("after", INK)],
            360.0,
            700.0,
        ),
        (
            "mixed flow",
            vec![
                Block::heading(1, "Bestiary", INK),
                Block::body(
                    "The creatures below are set in the house style, and the tables that follow \
                     them price the gear a party would carry out of the ruins.",
                    INK,
                ),
                sb(creature()),
                tb(equipment_table()),
                sb(sparse_creature()),
                tb(headerless_table()),
            ],
            300.0,
            400.0,
        ),
    ]
}

/// Re-derived from the pre-change engine. See the module docs before changing any of these.
///
/// **Moved once, deliberately, by spec 0060.** Forbidding a ragged line to be drawn past its
/// measure is a typographic correction, and a stat block is set ragged, so six of these ten
/// fixtures move. The four table cases that do not move are informative rather than incidental:
/// their cells are short enough never to have been over-measure, which is what a targeted change
/// should look like.
///
/// | fixture | before 0060 | after 0060 |
/// |---|---|---|
/// | statblock whole | `0xbe2e46dbbdfb9c85` | `0x47192e873263f5e9` |
/// | statblock narrow | `0x0797d2697c5a55d8` | `0xa7e4e0404a9e2d50` |
/// | statblock absent sections | `0x815ec8aa643567fd` | `0x2326d60454e58af1` |
/// | statblock split | `0x63ef8f8cf1431973` | `0x61d81882312fbcbf` |
/// | table wrapped | `0x3d748c61c283efa3` | `0x2f8d138d61822df3` |
/// | mixed flow | `0xd699cc419ccbd22e` | `0xd75f5e980a5b34ad` |
/// | table whole / split / headerless / empty | unchanged | unchanged |
const EXPECTED: &[(&str, u64)] = &[
    ("statblock whole", 0x4719_2e87_3263_f5e9),
    ("statblock narrow", 0xa7e4_e040_4a9e_2d50),
    ("statblock absent sections", 0x2326_d604_54e5_8af1),
    ("statblock split", 0x61d8_1882_312f_bcbf),
    ("table whole", 0x3fc2_380d_a6ad_2084),
    ("table wrapped", 0x2f8d_138d_6182_2df3),
    ("table split", 0x5b2f_4672_bd73_b52f),
    ("table headerless", 0xcc4e_d109_8b17_8624),
    ("table empty", 0x3acc_4ba6_c367_1efe),
    ("mixed flow", 0xd75f_5e98_0a5b_34ad),
];

/// The same corpus under the *complete* `Debug` rendering, including spec 0063's `spans` and
/// `run_colors`.
///
/// Two sets rather than one, because they answer two different questions. `EXPECTED` is the
/// geometry, and its constants have not moved since spec 0060 — proving 0063 changed no placed
/// position, size or text. This set is the structure, and it keeps the digest's original virtue:
/// a field nobody thought to list still cannot slip past it.
const EXPECTED_FULL: &[(&str, u64)] = &[
    ("statblock whole", 0xe9a9_a381_1b7c_34ef),
    ("statblock narrow", 0xd7bf_4757_ae84_c047),
    ("statblock absent sections", 0x1ec5_0338_61b5_8c77),
    ("statblock split", 0xfe2d_77a6_b6d0_a629),
    ("table whole", 0x0357_776a_51dd_b24e),
    ("table wrapped", 0xf54e_298b_4314_9dee),
    ("table split", 0xe0d8_c23c_47c8_a326),
    ("table headerless", 0x1e98_6c0e_678a_da77),
    ("table empty", 0x20a5_06c7_2af7_0c53),
    ("mixed flow", 0x22c8_b796_2e79_347a),
];

#[test]
fn the_bundled_components_produce_byte_identical_geometry() {
    let expected: std::collections::BTreeMap<&str, u64> = EXPECTED.iter().copied().collect();
    let full_expected: std::collections::BTreeMap<&str, u64> =
        EXPECTED_FULL.iter().copied().collect();
    let mut drift: Vec<String> = Vec::new();
    for (name, content, w, h) in corpus() {
        let pages = lay(content, w, h);
        let got = digest_pre_0063(&pages);
        match expected.get(name) {
            Some(&want) if want == got => {}
            Some(&want) => drift.push(format!("  {name}: expected {want:#x}, got {got:#x}")),
            None => drift.push(format!("  {name}: no expectation recorded, got {got:#x}")),
        }
        let full = digest(&pages);
        match full_expected.get(name) {
            Some(&want) if want == full => {}
            Some(&want) => drift.push(format!(
                "  {name} (structure): expected {want:#x}, got {full:#x}"
            )),
            None => drift.push(format!(
                "  {name} (structure): no expectation recorded, got {full:#x}"
            )),
        }
    }
    assert!(
        drift.is_empty(),
        "component geometry moved:\n{}\n\nRe-derive by the procedure in this file's module docs; \
         do not paste in what the test printed.",
        drift.join("\n")
    );
}

/// The corpus is only worth what it covers. Asserted rather than trusted, because a fixture list
/// that quietly stops exercising the split path is a fixture list that passes for the wrong
/// reason.
#[test]
fn the_corpus_exercises_the_split_paths() {
    let by_name: std::collections::BTreeMap<&str, (Vec<Block>, f32, f32)> = corpus()
        .into_iter()
        .map(|(n, c, w, h)| (n, (c, w, h)))
        .collect();
    for name in ["statblock split", "table split", "mixed flow"] {
        let (content, w, h) = by_name[name].clone();
        assert!(
            lay(content, w, h).len() > 1,
            "`{name}` must span more than one page, or it is not exercising a cut"
        );
    }
}
