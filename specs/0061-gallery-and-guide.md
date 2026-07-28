# Spec 0061 — A starter gallery and the pack authoring guide

**Milestone:** M4 · **Size:** small · **Status:** implemented

## Problem

Specs 0054–0057 built a format. A format with no worked example and no guide is a format nobody
outside this repository can use — and the milestone's whole point is *one person's work being usable
by another*.

Documentation is last because a guide to a format that is still moving is a guide that will be
wrong.

## What this builds

### `docs/pack-authoring.md`

The guide. It covers the manifest, component definitions (sections, shapes, rules, splitting,
colour), instances, requirements and version resolution, the commands, and the list of things that
deliberately refuse.

**It states the executable-extension decision and why**, in its own section rather than as an aside.
That paragraph is the reason the format is declarative, and a guide that loses it leaves the next
person to re-derive the argument — or, worse, to assume there wasn't one. It is asserted by a test
that greps for the load-bearing sentences.

**Every JSON example is parsed by a test** (`crates/core-model/tests/pack_guide.rs`), the anti-drift
precedent specs 0030, 0043 and 0053 all use. A reader who copies an example that no longer compiles
concludes the *tool* is broken.

Each fenced block carries its type in the info string — ` ```json component-def ` — and the test
parses it as that type. An **untagged** block is a failure, not a skip: an example nobody is checking
is exactly the one that rots.

The test also asserts that the guide's `move` definition is *equal to* the one in the shipped pack,
because a reader will copy the guide's. It earned that on the first run: the guide had `": "` where
the pack had `": "`.

### `examples/packs/`

Two worked examples, both installable as-is:

- **`grimdark.json`** — a house style. A 6×9 template with a baseline grid (spec 0058), a full type
  scale, and a restyled stat block with a heavier panel. The shape most packs will be.
- **`pbta-moves.json`** — a component pack. A `move` — a name, a trigger, and outcomes by roll —
  which no Rust type in this repository can express. It ships no template: it is a vocabulary, not a
  design.

Shipped as `pack.json` **manifests** rather than as zipped `.qpack` binaries. A repository of
permissively-licensed source should not carry opaque archives that cannot be reviewed in a diff, and
a pack's whole content is its manifest.

That in turn is why `quill pack info` and `quill pack install` now accept a bare `pack.json` as well
as a `.qpack`. It is not a concession to the examples: it is what a pack *author* has on disk while
they are writing one, and requiring a zip between every edit and every look puts a build step where
none is needed.

## Acceptance criteria

- `docs/` gains a pack authoring guide whose **every** example is parsed by a test, with untagged
  examples failing.
- The guide states the executable-extension decision and why, asserted.
- At least two packs ship as worked examples, each installing (0056) and laying out.
- A document requiring both example packs resolves and lays out, asserted end to end.
- Each example pack's definitions resolve every style they name, from the pack's own stylesheet or
  the bundled one — a definition whose styles were left behind sets in the default face on every
  machine but its author's.
- The guide's `move` example and the shipped pack's do not drift.

## Non-goals

- A registry, an index, or anywhere to fetch packs *from*. See spec 0055's non-goals.
- A gallery of *rendered* pages in the repository. Binary artifacts that go stale silently are worse
  than none; the examples are runnable, which is stronger.
