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

/// The digest with every field added since spec 0060 textually removed.
///
/// The digest is deliberately `Debug`-based so a new field cannot slip past it — which means a new
/// field also moves it even when no geometry did. Stripping them recovers the pre-0063 rendering
/// exactly, so the old constant still has to match: that is what separates "the struct grew" from
/// "the layout moved", and it is the only reason the constants below could be re-derived.
///
/// Spec 0063 added `spans` and `run_colors`; spec 0064 added `run_formats`, `run_shifts`, `weight`
/// and `italic`. Every one of them is stripped here, and `EXPECTED` below has not moved since spec
/// 0060 — which is the whole claim: neither increment moved a placed position, size or text.
fn digest_geometry(pages: &[LaidOutPage]) -> u64 {
    let text = format!("{pages:?}");
    let text = regex_free_strip(&text, "spans: [", "]");
    let text = regex_free_strip(&text, "run_colors: [", "]");
    let text = regex_free_strip(&text, "run_formats: [", "]");
    let text = regex_free_strip(&text, "run_shifts: [", "]");
    let text = strip_scalar(&text, "weight: ");
    let text = strip_scalar(&text, "italic: ");
    digest_str(&text)
}

/// Remove every `name: value` scalar field, plus the `, ` that separated it from the next.
///
/// Textual, like `regex_free_strip` above, and sound for the same reason: the corpus is fixed and
/// contains no body text spelling a field name. A fixture that did would be caught immediately —
/// the geometry digest would move against a constant that has not moved since spec 0060.
fn strip_scalar(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(name) {
        out.push_str(&rest[..i]);
        let after = &rest[i + name.len()..];
        let j = after
            .find(", ")
            .expect("a scalar field is followed by another");
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    out
}

/// The digest with every **frame extent** — `x_pt`, `w_pt`, `h_pt` — textually removed.
///
/// This is how spec 0069's move was re-derived rather than pasted. That increment redefined a
/// placed frame as the ink a part draws instead of the slot it was laid into, so exactly three
/// numbers per placed rectangle were expected to move and nothing else: no text, no line break, no
/// `y_pt`, no colour, no run table, no block count. Strip those three and the digest becomes a
/// statement about everything the increment must *not* have touched — and the constants below were
/// computed on the pre-0069 engine and still match on the post-0069 one, for all ten fixtures.
///
/// It is the same discipline `digest_geometry` applies to a struct that grew a field, pointed at a
/// value that changed instead: prove what did not move, then read the new number off the change you
/// can account for.
fn digest_extent_free(pages: &[LaidOutPage]) -> u64 {
    let text = format!("{pages:?}");
    let text = strip_extent(&text, "x_pt: ");
    let text = strip_extent(&text, "w_pt: ");
    let text = strip_extent(&text, "h_pt: ");
    digest_str(&text)
}

/// Remove every `name: value` scalar, whether it is followed by another field (`, `) or closes its
/// struct (` }`). [`strip_scalar`] handles only the first, which is all its callers needed.
fn strip_extent(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(name) {
        out.push_str(&rest[..i]);
        let after = &rest[i + name.len()..];
        let sep = after.find(", ");
        let close = after.find(" }");
        rest = match (sep, close) {
            (Some(s), Some(c)) if s < c => &after[s + 2..],
            // Keep the ` }`: the struct still has to close, or two differently-shaped renderings
            // could collapse to the same string.
            (_, Some(c)) => &after[c..],
            (Some(s), None) => &after[s + 2..],
            (None, None) => panic!("a scalar field is followed by a separator or a close"),
        };
    }
    out.push_str(rest);
    out
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
/// **Moved once by spec 0060**, and once by **spec 0069**. Both moves are deliberate; neither was
/// pasted in from a failing run.
///
/// 0060 forbade a ragged line to be drawn past its measure. A stat block is set ragged, so six of
/// these ten fixtures moved and the four table cases did not — their cells are short enough never
/// to have been over-measure, which is what a targeted change should look like.
///
/// 0069 redefined a placed frame as the **ink a part draws** rather than the slot it was laid into,
/// so every fixture moves: a table cell now reports its text's advance instead of its column, a
/// panel section its ink instead of the panel's inner width, and a paragraph's box no longer
/// includes the space below it that draws nothing. The re-derivation is `EXPECTED_EXTENT_FREE`
/// below — strip the three numbers 0069 was allowed to move and *nothing* moved, on all ten
/// fixtures, which is what makes these ten new values readable rather than merely observed.
///
/// | fixture | before 0060 | after 0060 | after 0069 |
/// |---|---|---|---|
/// | statblock whole | `0xbe2e46dbbdfb9c85` | `0x47192e873263f5e9` | `0x07a8a769b9253a17` |
/// | statblock narrow | `0x0797d2697c5a55d8` | `0xa7e4e0404a9e2d50` | `0x2eab4a8fd91fbb4f` |
/// | statblock absent sections | `0x815ec8aa643567fd` | `0x2326d60454e58af1` | `0xc3462392b3a5cd7d` |
/// | statblock split | `0x63ef8f8cf1431973` | `0x61d81882312fbcbf` | `0x740024b00b9e96f1` |
/// | table whole | `0x3fc2380da6ad2084` | unchanged | `0x7a49f1494c54de99` |
/// | table wrapped | `0x3d748c61c283efa3` | `0x2f8d138d61822df3` | `0x5c5910b35894e97f` |
/// | table split | `0x5b2f4672bd73b52f` | unchanged | `0xba87c2e5e18dcf44` |
/// | table headerless | `0xcc4ed1098b178624` | unchanged | `0x9b3937e64f1d3364` |
/// | table empty | `0x3acc4ba6c3671efe` | unchanged | `0xb238c77d431154aa` |
/// | mixed flow | `0xd699cc419ccbd22e` | `0xd75f5e980a5b34ad` | `0xbc914d0bce8deff0` |
const EXPECTED: &[(&str, u64)] = &[
    ("statblock whole", 0x07a8_a769_b925_3a17),
    ("statblock narrow", 0x2eab_4a8f_d91f_bb4f),
    ("statblock absent sections", 0xc346_2392_b3a5_cd7d),
    ("statblock split", 0x7400_24b0_0b9e_96f1),
    ("table whole", 0x7a49_f149_4c54_de99),
    ("table wrapped", 0x5c59_10b3_5894_e97f),
    ("table split", 0xba87_c2e5_e18d_cf44),
    ("table headerless", 0x9b39_37e6_4f1d_3364),
    ("table empty", 0xb238_c77d_4311_54aa),
    ("mixed flow", 0xbc91_4d0b_ce8d_eff0),
];

/// The corpus with `x_pt`, `w_pt` and `h_pt` stripped out of every placed rectangle — **the
/// derivation behind spec 0069's move**, and the reason `EXPECTED`'s ten new values above are a
/// claim rather than an observation.
///
/// Each of these was computed on the **pre-0069** engine, with this exact file compiled against it,
/// and every one of the ten still matches on the post-0069 engine. So the increment changed the
/// extent of placed rectangles and demonstrably nothing else: not a character of text, not a line
/// break, not a `y_pt`, not a colour, not a run table, not the number or order of placed blocks.
/// A move that could not be bounded like this would not be safe to accept.
const EXPECTED_EXTENT_FREE: &[(&str, u64)] = &[
    ("statblock whole", 0x0611_6a98_795e_664d),
    ("statblock narrow", 0x7fd2_bd10_05ee_cba4),
    ("statblock absent sections", 0x54d4_d562_e2b2_0688),
    ("statblock split", 0x6b03_3cbb_708d_239d),
    ("table whole", 0x917e_a515_ba57_9c48),
    ("table wrapped", 0xa14c_28e9_2a36_ba19),
    ("table split", 0xf08b_4221_f46c_cb05),
    ("table headerless", 0xd432_6a09_1af7_2ff4),
    ("table empty", 0x28ad_d060_96e0_aaef),
    ("mixed flow", 0xd268_bb29_acfd_bbec),
];

/// The same corpus under the *complete* `Debug` rendering, including every field specs 0063 and
/// 0064 added.
///
/// Two sets rather than one, because they answer two different questions. `EXPECTED` is the
/// geometry, and its constants have not moved since spec 0060 — proving that neither 0063 nor 0064
/// changed a placed position, size or text. This set is the structure, and it keeps the digest's
/// original virtue: a field nobody thought to list still cannot slip past it.
///
/// It moved at spec 0064, and only because `PlacedBlock::Text` grew four fields — `run_formats`,
/// `run_shifts`, `weight` and `italic`. The evidence that nothing else moved is the *other* set
/// above: strip those four out of the `Debug` rendering and every pre-0064 constant still matches,
/// which it does.
///
/// It moved again at spec 0069, for the same reason `EXPECTED` did and by the same amount: the two
/// sets differ only in which fields are stripped, and 0069 moved a value both of them include.
const EXPECTED_FULL: &[(&str, u64)] = &[
    ("statblock whole", 0x28ed_1945_d5d4_7add),
    ("statblock narrow", 0x66c6_f45c_4305_7690),
    ("statblock absent sections", 0x46f5_8387_6fe0_dae3),
    ("statblock split", 0x4ba0_bd82_012d_4d7b),
    ("table whole", 0xc223_d1b3_01fa_f89d),
    ("table wrapped", 0x9670_a2f5_f1cd_1484),
    ("table split", 0xfa55_1696_f9e5_7f67),
    ("table headerless", 0x0371_eed5_b3c9_f471),
    ("table empty", 0xb2ad_a7ed_148a_0223),
    ("mixed flow", 0xaf64_d985_4c75_4bf7),
];

#[test]
fn the_bundled_components_produce_byte_identical_geometry() {
    let expected: std::collections::BTreeMap<&str, u64> = EXPECTED.iter().copied().collect();
    let full_expected: std::collections::BTreeMap<&str, u64> =
        EXPECTED_FULL.iter().copied().collect();
    let extent_free_expected: std::collections::BTreeMap<&str, u64> =
        EXPECTED_EXTENT_FREE.iter().copied().collect();
    let mut drift: Vec<String> = Vec::new();
    for (name, content, w, h) in corpus() {
        let pages = lay(content, w, h);
        // Checked first and reported first, because it is the informative one: if this moves, the
        // change is not the extent change it claims to be, and the other two digests cannot tell
        // you that.
        let extent_free = digest_extent_free(&pages);
        match extent_free_expected.get(name) {
            Some(&want) if want == extent_free => {}
            Some(&want) => drift.push(format!(
                "  {name} (extent-free): expected {want:#x}, got {extent_free:#x} — something \
                 other than a frame's x/w/h moved"
            )),
            None => drift.push(format!(
                "  {name} (extent-free): no expectation recorded, got {extent_free:#x}"
            )),
        }
        let got = digest_geometry(&pages);
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
