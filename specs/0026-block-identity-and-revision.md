# 0026 — Stable `BlockId`, document revision, and O(1) asset lookup

**Milestone:** M1 · **Status:** implemented

## Why

Incremental, dependency-tracked layout (spec 0031) has to answer one question cheaply: *is what I
cached for this block still valid?* That needs a name for "this block" which survives editing. Today
there is none — blocks are addressable only by their index in `Document::content`, so inserting a
paragraph at the top renumbers every block after it and invalidates a whole document's worth of
cache entries for a one-paragraph edit. An index is a position, not an identity.

Two smaller gaps travel with it:

- **No revision counter.** Nothing can ask "has this document changed since I last looked?" without
  diffing the whole tree.
- **Asset resolution is quadratic.** `measure_block` resolved an image's asset id with
  `assets.iter().find(..)`, and it runs *once per candidate frame per block*. On an art-heavy
  500-page book — the workload this engine exists for — that is O(blocks × assets).

## What

### `BlockId`

A `u64` newtype, `Copy + Eq + Hash + Ord`, added as a `#[serde(default)]` field to all three `Block`
variants, with `Block::id()` / `set_id()` and the constructors `Block::body/heading/image`.

`u64` rather than a string: this is a cache key on the hot path, so `Copy` and allocation-free
hashing is what it wants, and it serializes as a plain number so the manifest stays readable and
git-diffable. `BlockId(0)` is `UNASSIGNED` — the state of a block built in memory or loaded from a
manifest written before ids existed. (Worth noting: nothing else in `core-model` derives `Eq`/`Hash`,
because every geometry field is `f32`.)

`Document::assign_missing_block_ids` runs on every load, giving unidentified blocks ids in document
order and leaving already-identified ones alone.

**A duplicate id is refused, not repaired.** Two blocks claiming one identity means a cache lookup
can return the wrong block's layout, and silently renumbering one of them breaks whichever external
reference pointed at the block that moved. `LoadError::DuplicateBlockId`.

**`next_block_id` is persisted.** Without it, a reload would rewind the allocator and hand a deleted
block's id to a new block — which is worse than having no ids at all, because the new block would
silently inherit the old one's cache entry.

### Revision counter

`Document::revision`, monotonic, bumped by `bump_revision()`. Persisted, so a reload does not reset
what a cache may have keyed against.

### Asset index

`Document::asset_index()` returns an id → `&Asset` map, and `lay_out_in_thread` builds one per
layout pass and shares it with every `measure_block` call. The public signatures still take
`&[Asset]`, so no caller changed.

### No format bump

`id`, `revision` and `next_block_id` are all `#[serde(default)]`, so a v1 manifest still loads and
`FORMAT_VERSION` stays 1. The first bump is spec 0030.

## Acceptance criteria

- [x] `Block::id()` works for all three variants; every block in a loaded document has an assigned id.
- [x] A manifest with no ids gets `1, 2, 3` in document order, and those ids are unchanged across a save/reload.
- [x] Inserting a block at the front leaves every existing block's id untouched, and the new id is fresh.
- [x] Two blocks sharing an id fail with `LoadError::DuplicateBlockId(id)`.
- [x] A manifest mixing assigned and unassigned ids keeps the assigned one and does not collide with it.
- [x] 1000 minted ids are distinct, disjoint from live ids, and never `0`.
- [x] A reload does not rewind the id allocator.
- [x] `revision` increases strictly and survives a round trip.
- [x] Layout is behaviorally unchanged: every pre-existing layout-engine test passes untouched.
- [x] 2,000 image blocks against 2,000 assets all place through the index (correctness at scale; the timing claim belongs to spec 0027's bench, not to a unit test on a shared runner).
- [x] An unknown asset id is still skipped without panicking, and the block after it still places.
- [x] A manifest predating ids still loads with `FORMAT_VERSION` unchanged.

## Note: the exported PDF's identifier changes

`writer::doc_id_bytes` hashes `doc.to_json()` into the document identifier, so adding a field to
every block necessarily moves it. Diffed against the previous build to confirm the change is *only*
that: exactly 120 bytes differ, in three places — the XMP `DocumentID`, the XMP `InstanceID`, and the
trailer `/ID`, all three derived from that one hash. Total length is unchanged at 8558 bytes, and
every page content stream, font and image byte is identical. `SAMPLE_EXPORT_DIGEST` is updated with
that evidence recorded beside it.

This is the byte-parity tripwire from spec 0025 doing its job: it fired, the change was examined, and
it turned out to be the expected one. A silent change here would have been indistinguishable from a
regression.

## Non-goals

- Using the ids for anything. The measurement cache that consumes them is spec 0031; this increment
  only lays the substrate, which is why it ships separately and small.
- Ids on anything other than content blocks (assets already have string ids; frames and master pages
  arrive with spec 0030).
- Any editing API. `bump_revision` exists and is tested, but nothing mutates a document yet.
