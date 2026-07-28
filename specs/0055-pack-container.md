# Spec 0055 — The `.qpack` container

**Milestone:** M4 · **Size:** medium · **Status:** implemented

## Problem

Spec 0054 made a component declarable. A declaration that cannot leave the document it was written
in is not an ecosystem: a house style, a set of masters, a game system's stat-block layout all still
exist only as whatever the author happened to build.

## What this builds

`.qpack` — a versioned bundle of templates, styles, component definitions and assets, with a
manifest that carries **provenance**.

It follows spec 0025's `.tpub` precedent exactly — a zip, a manifest, a version integer, a typed
load contract, a zip-slip refusal — because a pack is the same *kind* of object, and a second,
differently-shaped container would be a second thing to get wrong.

```text
pack.json          # the manifest: identity, provenance, and all declarative content
assets/            # linked originals the pack's templates and components reference
```

Everything declarative lives inline in `pack.json` rather than in `templates/*.json` +
`components/*.json` subdirectories. One manifest is one parse, one version gate and one round-trip
test; a directory of files is a filesystem layout to validate, a partial-read state to define, and
a second place for a name to disagree with a filename.

### The manifest

```rust
pub const PACK_VERSION: u32 = 1;

pub struct PackManifest {
    pub pack_version: u32,       // the *format*; a newer one is a refusal
    pub name: String,            // slug, how a document names it
    pub title: String,
    pub description: String,
    pub version: String,         // the *pack's own* content version, e.g. "1.2.0"
    pub source: String,          // required, non-empty
    pub license: String,         // required, non-empty
    pub templates: Vec<Template>,
    pub styles: StyleSheet,
    pub components: BTreeMap<String, ComponentDef>,
    pub assets: Vec<Asset>,
}
```

Two versions, deliberately. `pack_version` is the container format and is quill's; `version` is the
pack's own, and is the author's — a publisher shipping "Ashen Vault 1.2.0" is not making a claim
about quill's format. Spec 0056 resolves against the second and refuses on the first.

**`source` and `license` are required and non-empty.** Content arriving from a stranger with no
provenance is content nobody should install, and a field that may be empty is a field that will be.
The refusal is a typed error naming the pack and the missing field, checked on read *and* on write —
a pack that cannot be installed should not be writable either.

### Errors

New `LoadError` variants, each distinct because an error has to name what is wrong:

| Variant | Cause |
|---|---|
| `PackParse(String)` | `pack.json` is not well-formed, or does not match the schema |
| `UnsupportedPackVersion { found, supported }` | a `pack_version` newer than this build |
| `NotAQpack(String)` | a readable zip with no `pack.json` |
| `PackMissingProvenance { pack, field }` | `source` or `license` empty |
| `PackUnsafePath { pack, path }` | an asset path that is absolute or escapes via `..` |

The unsafe-path check is stricter than `.tpub`'s. A `.tpub` refuses a zip *entry* that escapes on
extraction; a `.qpack` refuses that **and** an `Asset::path` inside the manifest that is absolute or
traverses, because a pack is the one artifact in this system that routinely comes from someone
else. It is checked when the manifest is read, before a single byte is written anywhere.

### The CLI

`quill pack` becomes the pack-format command group, starting with `quill pack info <file.qpack>`,
which prints identity, provenance and contents.

The existing `.tpub` writer moves from `quill pack` to **`quill tpub`**. It was always named for the
verb rather than the artifact; it makes a `.tpub`, and `pack` now means the pack format the roadmap
reserves it for. CI is updated in the same commit.

## Acceptance criteria

- A pack round-trips: written, read back, equal.
- **Every bundled template exports as a pack that reloads identically** — spec 0053's precedent: a
  format that round-trips the struct but builds a different document proves nothing, so the test
  compares the *documents* the templates build, not just the templates.
- Malformed, newer-versioned and missing-manifest packs each fail with a distinct typed error.
- A pack may not carry an absolute path or a `..` traversal in an asset path — asserted in the
  manifest *and* on extraction.
- Licence and source are required, non-empty, and surfaced by `quill pack info`.
- A pack's component definitions are validated on read, by spec 0054's rules.

## Non-goals

- **Executable content.** See `docs/roadmap.md`.
- Signing, checksums, or a registry. `source` is provenance a human reads, not a trust root. When
  there is somewhere to fetch packs *from*, that is when a signature means something.
- Dependencies between packs. Spec 0056 resolves a flat set by name and version.
