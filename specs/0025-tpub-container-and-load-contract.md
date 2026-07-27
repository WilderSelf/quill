# 0025 — `.tpub` container + versioned load contract

**Milestone:** M1 · **Status:** implemented

## Why

Three gaps, all in the same place — how a document gets from disk into memory — and each one blocks
the increments that follow.

**1. The versioning contract was documentation only.** `docs/format-spec.md` has always said readers
"reject formats newer than they understand and migrate older ones forward". Nothing implemented it:
`Document::from_json` deserialized any `format_version` silently. A document written by a future
Quill would load with its unrecognized fields dropped — and could then be saved back over the
original, destroying the parts this build did not understand. Every increment from 0026 onward
changes the serialized shape, so the gate has to exist *before* the first change, or v1 files become
undiagnosable after the fact.

**2. Nothing owned "the document's asset root".** `Asset::path` is a relative path, and the two
crates that resolve it disagreed about what it was relative to: `export-pdf` hardcoded
`Path::new(".")` (the *process working directory*), while `quill-render` took a `base_dir` argument
from its caller. So exporting the same document from a different directory silently dropped its
images — precisely the failure class `CLAUDE.md` forbids, since a dropped image is not visible in
the PDF until someone looks.

**3. There was no container.** `.tpub` was described in the format spec and named in CLI help text,
but the only thing that existed was a bare `document.json`. Assets and fonts had no defined home, so
a document was not actually portable.

## What

### Versioned load contract

`Document::from_json` returns `Result<Document, LoadError>` instead of leaking `serde_json::Error`.
The manifest being JSON is an implementation detail of the format; callers matching on a `serde`
error type would make the encoding impossible to change later.

Version handling runs on the untyped `serde_json::Value` *before* deserialization, because an older
manifest by definition does not fit the current `serde` types — that is what makes it old:

| Manifest `format_version` | Behavior |
|---|---|
| absent | treated as current; tolerated so hand-written test manifests can be short |
| `< FORMAT_VERSION` | migrated forward through a chain, one arm per version step |
| `== FORMAT_VERSION` | loaded as-is |
| `> FORMAT_VERSION` | **refused** with `LoadError::UnsupportedVersion { found, supported }` |

The migration chain is empty today (nothing is older than v1). The shape is in place so the first
real bump — spec 0030's `FORMAT_VERSION` 2 — is an addition rather than a redesign, and so `migrate`
is already on every load path when that bump lands.

### The container

`Tpub::write` / `Tpub::open_into` / `Tpub::read_manifest` over a deflate-only zip holding
`document.json` plus `assets/`, `fonts/`, `thumbnails/`.

Opening **extracts to a caller-named directory** and returns `OpenedTpub { document, asset_root }`.
Two decisions worth stating:

- *Why extract at all, rather than stream from the zip.* Both consumers (`export-pdf`'s image
  decode, `render`'s proxy cache) already take a `&Path` base directory and want ordinary file
  reads. A zip-streaming shim would have to be threaded through every call site to buy nothing.
- *Why the caller names the directory.* A hidden temp directory would be invisible state with no
  owner and no defined lifetime. Making the caller name it keeps "where are this document's assets
  right now" an answerable question. The CLI extracts `book.tpub` to `book.tpub.d/`, so repeated
  opens are idempotent.

`read_manifest` reads the manifest without extracting anything, which is what makes it possible to
refuse a too-new document *before* writing its payload across the filesystem.

**Zip-slip is refused, not sanitized.** A container entry named `../../etc/foo` would escape the
extraction directory. Entry names containing `..`, a root, or a drive prefix are rejected on both
read and write. Rewriting such a path to something safe would extract a file the document never
described, and a `.tpub` that tried to escape is not a document to be repaired.

### Asset root

`ExportOptions::asset_root` replaces the hardcoded `Path::new(".")`. It defaults to `.`, so a caller
that never sets it sees exactly the previous behavior. The CLI now sets it: a `.tpub`'s extracted
root, or — for a bare `document.json` — the directory holding that file, rather than wherever the
process happens to be running.

### `quill pack`

A container the headless entrypoint can read but never write would leave the format unreachable from
the only interface that exists today. `quill pack <document.json> -o <out.tpub>` writes the manifest
plus every linked asset, resolved against the document's own directory.

An asset that cannot be read is a **hard failure**, not a warning. A `.tpub` exists to be portable;
quietly packing a container around a missing image produces a document that is broken everywhere
else it is opened, and the point of failing at pack time is that this is the last moment the
original file is still at hand.

## Acceptance criteria

- [x] A manifest declaring a newer `format_version` is refused with `UnsupportedVersion { found, supported }`, not silently downgraded.
- [x] An absent `format_version` migrates forward to `FORMAT_VERSION`.
- [x] Malformed JSON yields `LoadError::Parse`; the public signature no longer exposes `serde_json::Error` (asserted by binding the error to `LoadError`).
- [x] `Tpub::write` then `Tpub::open_into` round-trips a document `assert_eq!`-equal to the input, with `asset_root.join(asset.path)` present and byte-identical.
- [x] A `.tpub` whose manifest declares a too-new version fails with `UnsupportedVersion` **and extracts no files**.
- [x] A zip with no `document.json` yields `NotATpub`; a missing file yields `Container`, not a panic.
- [x] Container entries that escape the extraction root are refused, on both read and write.
- [x] `read_manifest` extracts nothing (asserted by counting directory entries afterwards).
- [x] Exporting `Document::sample()` is **byte-identical to the pre-change build** — verified by building `main` in a worktree and `cmp`-ing the two PDFs, and pinned going forward by a digest constant.
- [x] A *relative* asset path resolves against `asset_root` and embeds; a wrong `asset_root` skips the image rather than guessing at another file.
- [x] The `zip` dependency is declared once in `[workspace.dependencies]`, permissive (MIT), with `default-features = false`.
- [x] `quill pack` writes a `.tpub` that `quill export` then opens and exports from, end to end; an unreadable asset fails the pack rather than producing a broken container.

## Notes from implementation

Two findings worth recording, both caught by the byte-parity check rather than by reasoning.

**Adding `zip` silently changed the exported PDF.** `zip`'s umbrella `deflate` feature expands to
`deflate-zopfli + deflate-flate2-zlib-rs`, and that `flate2/zlib-rs` switches `flate2`'s compression
backend for *every* crate in the workspace through feature unification — including `export-pdf`'s
`/FlateDecode` streams. The exported sample grew by 24 bytes with an identical `/Length1`: same font
program, differently compressed. The fix is to select `deflate-flate2` specifically, reusing the
`flate2` already in the graph with its default backend. Output is byte-identical to `main` again,
and the lock file gains only `zip` and `typed-path`. Cross-crate feature unification can reach the
press-output path from a dependency that has nothing to do with it.

**Press exports are not byte-reproducible across time when the OutputIntent profile is synthesized.**
`synth_cmyk_profile()` stamps the current time into the ICC header (`dateTimeNumber`, header bytes
24..36), and PDF/X embeds that profile verbatim. This is inherent to ICC, not a defect — a real
export supplies a fixed press profile and *is* reproducible. But it means the digest test has to zero
that one field, or it would measure the clock instead of the writer. A companion test asserts the
timestamp is really there, so the normalization step does not outlive its reason.

## Non-goals

- Saving *back* to a `.tpub` from an edited in-memory document, including what happens to assets
  added or removed during editing. Writing a container is implemented; incremental save is not.
- Persisting the extracted working directory across sessions, or caching proxies alongside it.
- Any change to `FORMAT_VERSION`, which stays 1. The first bump is spec 0030.
