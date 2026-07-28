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
use quill_core_model::{Asset, Block, BlockId, Color, StatBlock, StyleSheet, Table};
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
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
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

fn creature() -> StatBlock {
    StatBlock {
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
fn sparse_creature() -> StatBlock {
    StatBlock {
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
    let sb = |stat: StatBlock| Block::StatBlock {
        id: BlockId::UNASSIGNED,
        stat,
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
const EXPECTED: &[(&str, u64)] = &[
    ("statblock whole", 0xbe2e_46db_bdfb_9c85),
    ("statblock narrow", 0x0797_d269_7c5a_55d8),
    ("statblock absent sections", 0x815e_c8aa_6435_67fd),
    ("statblock split", 0x63ef_8f8c_f143_1973),
    ("table whole", 0x3fc2_380d_a6ad_2084),
    ("table wrapped", 0x3d74_8c61_c283_efa3),
    ("table split", 0x5b2f_4672_bd73_b52f),
    ("table headerless", 0xcc4e_d109_8b17_8624),
    ("table empty", 0x3acc_4ba6_c367_1efe),
    ("mixed flow", 0xd699_cc41_9ccb_d22e),
];

#[test]
fn the_bundled_components_produce_byte_identical_geometry() {
    let expected: std::collections::BTreeMap<&str, u64> = EXPECTED.iter().copied().collect();
    let mut drift: Vec<String> = Vec::new();
    for (name, content, w, h) in corpus() {
        let got = digest(&lay(content, w, h));
        match expected.get(name) {
            Some(&want) if want == got => {}
            Some(&want) => drift.push(format!("  {name}: expected {want:#x}, got {got:#x}")),
            None => drift.push(format!("  {name}: no expectation recorded, got {got:#x}")),
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
