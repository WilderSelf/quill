# 0034 — egui app shell

**Milestone:** M1 · **Status:** implemented

## Why

`crates/app` was a 17-line `println!` stub whose only dependency was `quill-core-model`. There was
no window, no viewport, no state — nothing for a renderer to draw into, and no place where a
keystroke could become a re-flow.

Everything M1 built ends here: the container opens a document, the model carries its styles and
master pages, `LayoutSession` re-flows it incrementally, `quill-fonts` measures it and the paint
list draws it. This is the increment where those become an application.

## What

### A library, plus a thin binary

Everything interesting about an editor — opening a document, mapping the viewport, deciding what to
paint, routing an edit into incremental relayout — is *logic*, not windowing. In a library it is
ordinary unit-testable code that runs on every leg of the CI matrix with **no display server, no
GPU and no system libraries**.

That split is what lets the shell's behavior be gated by CI at all. A shell that lived only inside
`eframe::run_native` could be verified by compiling and by nothing else.

`eframe` sits behind a non-default `gui` feature, and the binary declares
`required-features = ["gui"]`, so the default workspace build needs no windowing stack.

### The shell

| Concern | Behavior |
|---|---|
| Opening | `.tpub` via the container, or the built-in sample with no file |
| Broken links | The document still opens; unresolved images are counted and surfaced in the status bar |
| Viewport | Scroll offset, height and zoom in points; `doc_to_screen` / `screen_to_doc` |
| Virtualization | `visible_pages` bounds painting to what is on screen |
| Editing | `edit_text` routes through `LayoutSession` and returns exactly the pages to repaint |

### The constraint the shell itself could break

`CLAUDE.md`'s central claim is that a 500-page art-heavy book stays smooth. A viewport that walks
the whole document each frame violates that **in the UI**, no matter how fast the engine is — so
`visible_pages` and a `painted_pages` counter exist specifically so a test can prove it doesn't:
scrolled to the last page of a long document, at most a handful of pages are painted.

Likewise `EditOutcome::repaint` is spec 0031's `changed_pages` passed straight through. A keystroke
repaints a bounded number of pages, asserted.

## Acceptance criteria

- [x] `quill-app` has a lib target, and its 12 tests run with no display server and no GPU on all three OSes.
- [x] Opening lays the document out; the built-in sample opens with no file.
- [x] Viewport math round-trips within 0.01 pt at zoom 0.25, 1.0 and 4.0.
- [x] Scrolled to the end of a long document, **only visible pages are painted** — `painted_pages` equals the visible count, not the page count.
- [x] The visible range is never empty, even scrolled far past the end. A viewport bug should not produce a blank window.
- [x] Painting emits one page op per visible page.
- [x] An edit repaints ≤ 3 pages of a multi-page document and reuses the rest.
- [x] An edit changes the text **and preserves the block's identity** — losing the id would orphan every cache entry keyed on it.
- [x] Editing an image block is a no-op rather than silently replacing it with a text block.
- [x] A document with a missing linked asset still opens, with the skip reported.
- [x] A restyle re-flows and repaints — which also exercises spec 0031's context fingerprint from the UI side.
- [x] `cargo clippy --all-targets --all-features -D warnings` is clean *including* the GUI binary, and `cargo test --workspace --all-features` passes (283 tests).

## The `--all-features` trap

`cargo clippy --all-targets --all-features` enables the `gui` feature, which pulls
`eframe`/`winit`/`glow`. macOS and Windows ship their windowing libraries; **Linux does not**, and
the ubuntu leg is a *required* branch-protection context — so without an apt step, adding an
optional GUI dependency reddens a required check for reasons that have nothing to do with the code.

The CI `check` job now installs the windowing headers on Linux only, guarded by
`if: runner.os == 'Linux'`.

## Non-goals

- **Rasterized text in the window.** The canvas currently draws text with egui's own font rather
  than through the tiny-skia outline rasterizer. What matters for correctness — *where* each line
  sits — comes from the same op list `quill render` rasterizes; wiring that raster into a GPU
  texture is a follow-up, and pretending otherwise would overstate what this increment verifies.
- Text editing affordances: carets, selection, hit-testing from a click back to a `BlockId`.
- Undo/redo. `Document::revision` exists and increments; nothing keeps a history yet.
- Saving. The container can be written (`Tpub::write`, `quill pack`) but the shell has no save path.
- Any preflight or export UI. Both are reachable from the CLI.
