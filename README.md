# Quill

> **Quill** is a working codename — the product name is still to be decided.

An open-source, cross-platform desktop publishing app for **semi-professional hobbyist TTRPG
publishers** — art-heavy game books up to ~500 pages that need **press-ready PDF/X** output for
print-on-demand (DriveThruRPG, Lulu, IngramSpark).

It aims to fill a real gap: today's tools force a choice between *easy but not print-grade*
(Homebrewery, GM Binder) and *print-grade but expensive/hard/slow-on-long-documents* (InDesign,
Affinity Publisher, Scribus). Quill targets a **hybrid**: an easy structured-content on-ramp
that flows into a real frame/master-page layout engine with correct PDF/X export — fast enough
to stay smooth at 500 pages.

## Status

**Pre-alpha. Milestones M0–M4 are complete**; there is no graphical editor yet, which is what
"pre-alpha" means here — the pipeline is real and the shell is not.

- **M0** — press output. Headless PDF/X-1a / PDF/X-3 export, gated in CI by a Ghostscript
  well-formedness check. One item stays open and is not automatable: a real print-on-demand upload
  validated against a B2A-equipped CMYK profile.
- **M1** — the editing core. Shaping, Knuth-Plass justification, hyphenation, threaded text frames,
  master pages, incremental dependency-tracked layout, and a benchmark gate that keeps a 500-page
  document smooth.
- **M2** — the beginner on-ramp. A Markdown-ish source imports to a templated book with stat blocks,
  tables, a generated contents list and PDF bookmarks.
- **M3** — pro polish. Blocks split across frames, tables and stat blocks continue, preflight checks
  a printer's own numbers against the geometry actually placed, and a second export profile carries
  clickable links while the press file stays provably annotation-free.
- **M4** — the ecosystem. A component is a *declaration* rather than a Rust type, and `.qpack`
  content packs share templates, styles and component definitions between publishers. Plus a
  baseline grid, and one hyphenator shared by screen and press.

The plan of record — milestones, increment order, decisions, known issues and open questions — is
[`docs/roadmap.md`](docs/roadmap.md). Architecture and the non-negotiable print constraints are in
[`CLAUDE.md`](CLAUDE.md).

### A note on what a content pack is

A pack carries templates, styles, component definitions and assets. It carries **no code**, and that
is a deliberate design decision rather than a missing feature — the reasoning is in
[`docs/pack-authoring.md`](docs/pack-authoring.md).

## Building

Requires a Rust toolchain (install via [rustup](https://rustup.rs)).

```bash
cargo build            # build all crates
cargo test --workspace # run tests
cargo run -p quill-cli -- --help
```

A first book, headless:

```bash
cargo run -p quill-cli -- new --list                        # bundled templates
cargo run -p quill-cli -- import book.md --output book.tpub --template rulebook
cargo run -p quill-cli -- preflight book.tpub --preset generic
cargo run -p quill-cli -- export book.tpub --output book.pdf --icc press.icc
cargo run -p quill-cli -- render book.tpub --output page0.png   # what the canvas would draw
```

Content packs:

```bash
cargo run -p quill-cli -- pack install examples/packs/pbta-moves.json
cargo run -p quill-cli -- pack list
```

## Workspace layout

Layered Rust crates under `crates/` (`core-model`, `text-layout`, `layout-engine`, `fonts`,
`color`, `render`, `export-pdf`, `components-ttrpg`, `app`, `cli`, `testdoc`). The pipeline can be exercised
headless via `quill-cli` before the UI exists. See `CLAUDE.md` for architecture and the
non-negotiable print-output constraints.

## Development method

Spec-driven: non-trivial features start from a markdown spec under `specs/` (indexed by
`specs/README.md`) that defines behavior and acceptance criteria before code is written. Sixty-odd
specs so far; the spec is the source of truth and is revised when behavior changes.

Two conventions worth knowing before changing layout. Anything that moves typography is checked
against a **corpus digest re-derived from the pre-change build** — never re-recorded from a failing
run — and both values are kept on the record. And the file format is versioned with a load contract
that refuses a newer file rather than half-loading it.

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
