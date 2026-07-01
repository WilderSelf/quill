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

**Early / pre-alpha (milestone M0).** The current focus is proving the differentiator — a
press-correct PDF/X export pipeline — end to end. See the roadmap in `CLAUDE.md` and the design
in the plan referenced there.

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
