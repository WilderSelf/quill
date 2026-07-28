# Quill

> **Quill** is a working codename — the product name is still to be decided.

An open-source, cross-platform **desktop publishing application** — long, art-heavy books up to
~500 pages that need **press-ready PDF/X** output for print-on-demand (Lulu, IngramSpark,
DriveThruRPG).

Quill is a general-purpose desktop publishing application first. Its flagship audience is
independent publishers of illustrated game books, and that is where its fixtures, its templates and
its print-on-demand presets come from — but every mechanism in it is a general one. A panelled
record, a range-lookup table, a two-column reference layout: a cookbook, a field guide, a hardware
manual and a thesis want each of those on exactly the same terms, and a mechanism that only one
genre can use is treated here as a defect rather than a feature.

It aims to fill a real gap: today's tools force a choice between *easy but not print-grade* and
*print-grade but expensive, hard, or slow on long documents* (InDesign, Affinity Publisher,
Scribus). Quill targets a **hybrid**: an easy structured-content on-ramp that flows into a real
frame/master-page layout engine with correct PDF/X export — fast enough to stay smooth at 500 pages.

## Status

**Pre-alpha.** Milestones M0–M3 are complete: a press-correct PDF/X-1a/X-3 export pipeline,
Knuth-Plass justification with hyphenation, frames and threading, multi-column, master pages,
incremental dependency-tracked layout, generated contents and PDF bookmarks, block fragmentation
across frames, POD presets and placed-geometry preflight. M4 — the general typographic core: inline
runs, character styles, lists, tab stops and a baseline grid — is decomposed and in progress.

The authoritative plan is [`docs/roadmap.md`](docs/roadmap.md).

## Building

Requires a Rust toolchain (install via [rustup](https://rustup.rs)).

```bash
cargo build            # build all crates
cargo test --workspace # run tests
cargo run -p quill-cli -- --help
```

## Workspace layout

Layered Rust crates under `crates/` (`core-model`, `text-layout`, `layout-engine`, `color`,
`render`, `export-pdf`, `components-ttrpg`, `app`, `cli`). The pipeline can be exercised
headless via `quill-cli` before the UI exists. See `CLAUDE.md` for architecture and the
non-negotiable print-output constraints.

## Development method

Spec-driven: non-trivial features start from a markdown spec under `specs/` (indexed by
`specs/README.md`) that defines behavior and acceptance criteria before code is written.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.

### Bundled fonts

`crates/export-pdf/assets/SourceSerif4-Regular.ttf` is **Source Serif 4** (a static `glyf`
instance), licensed under the
[SIL Open Font License, Version 1.1](crates/export-pdf/assets/SourceSerif4-LICENSE.txt)
(© 2014 The Source Serif 4 Project Authors). It ships as a data asset used as the default
embeddable body font for PDF/X export; the OFL is compatible with the project's MIT/Apache-2.0
code license.
