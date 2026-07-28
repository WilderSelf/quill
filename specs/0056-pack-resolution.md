# Spec 0056 — Pack resolution

**Milestone:** M4 · **Size:** medium · **Status:** implemented

## Problem

Spec 0055 can write and read a `.qpack`. Nothing installs one, nothing lists what is installed, and
no document can say which pack it needs. A pack that has to be manually merged into a manifest is a
zip file, not an ecosystem.

## What this builds

Three things: a place packs live, a way a document names the ones it needs, and a resolution step
that either produces a merged library or **refuses**.

### The pack root

A directory. Resolved in this order, first hit wins:

1. an explicit path (`quill pack install|list --packs <dir>`);
2. `$QUILL_PACKS`;
3. `$XDG_DATA_HOME/quill/packs`, else `$HOME/.local/share/quill/packs`; on Windows,
   `%APPDATA%\quill\packs`.

Hand-rolled rather than pulling in a platform-directories crate: it is a dozen lines against three
environment variables, and the dependency graph stays as permissive and as small as `CLAUDE.md`
asks. Layout inside:

```text
<root>/<name>/<version>/pack.json
<root>/<name>/<version>/assets/…
```

Name *and* version in the path, so two versions of a pack coexist and `list` is a directory walk
rather than an index file that can disagree with what is on disk.

### A document names what it needs

```rust
pub struct PackRequirement { pub name: String, pub version: String }

// on Document, #[serde(default, skip_serializing_if = "Vec::is_empty")]
pub requires: Vec<PackRequirement>,
```

Additive; `FORMAT_VERSION` stays 3.

### Version resolution — the rule, stated

`version` is either an **exact version**, or a **dotted prefix** of one:

- `"1.2.0"` matches only `1.2.0`.
- `"1.2"` matches `1.2.0` and `1.2.9`, not `1.3.0`.
- `"1"` matches every `1.x`.
- `""` matches any installed version.

Among matches the **highest** wins, compared component-by-component: numerically when both
components parse as integers, lexically otherwise. Prefix matching is on whole dotted components, so
`"1"` never matches `10.0.0` — the substring reading of that rule is the classic version-matching
bug and it is closed by construction.

No match is an **error naming the pack, the requested version, and what is actually installed**.
Never a fallback to a default style: a book that looks subtly wrong is worse than one that refuses
to open, which is `CLAUDE.md`'s first rule applied to somebody else's content.

### Merging, and the collision rule

Resolved packs contribute component definitions, paragraph styles and templates. Precedence, least
to most specific:

```
bundled  <  packs  <  the document's own
```

A pack defining `statblock` is a legitimate restyle of the bundled one; a document defining it beats
both, because the document is the thing being edited.

**Two resolved packs defining the same component name is an error naming both packs and the
component** — not last-one-wins. Which pack won would depend on `BTreeMap` iteration order, i.e. on
the packs' names, which is a coin flip a publisher cannot see, debug or predict. The same rule
applies to templates. Paragraph *styles* are deliberately exempt and merge last-writer-wins in name
order: a style name is a shared vocabulary (`body`, `h1`) that packs are expected to co-define, and
refusing there would make any two packs uninstallable together.

### The CLI

- `quill pack install <file.qpack> [--packs <dir>]` — validate, then extract to
  `<root>/<name>/<version>/`. Refuses to overwrite a different pack already at that path unless
  `--force`.
- `quill pack list [--packs <dir>]` — name, version, source and licence per installed pack.
- `quill pack info <file.qpack>` — from spec 0055.

Every command that loads a document resolves its `requires` first and fails on a refusal. Document
commands take the root from `$QUILL_PACKS` or the platform default rather than growing a `--packs`
flag apiece: a document's packs are a property of the machine it is being built on, not of the
invocation, and a per-command flag invites two commands in one session to disagree about which pack
a book was set in.

Resolution is a **document transformation** — `Document::apply_packs` folds the pack's definitions
and styles in and clears `requires` — rather than an argument threaded into layout. If layout took
an optional pack set, its default would be "no packs", and a caller who forgot would lay the book
out in the default face: the silent fallback this spec exists to prevent. Instead a document either
has been resolved or has not, and `lay_out` **panics** on one that has not, naming the packs. A
caller skipping resolution is a programming error, and the panic is the same visible-failure posture
`lay_out_with_template` already takes for a template that hands back a page with no frames.

## Acceptance criteria

- A `.tpub` naming a pack that is not installed fails to lay out with a typed error naming the pack
  and its version.
- The version rule above is asserted, prefix cases included, and `"1"` is asserted **not** to match
  `10.0.0`.
- Two packs defining the same component name is an error naming both.
- `quill pack list` shows name, version, source and licence for each installed pack.
- A document laid out under a resolved pack uses the pack's definitions and styles; the document's
  own win over the pack's, and the pack's over the bundled — asserted on *placed geometry*, not on
  the merged map, because merging that does not move the page has not been demonstrated.
- Laying out an unresolved document is refused rather than defaulted.
- `requires` survives a manifest round trip, and a document with none writes none.

## Non-goals

- Fetching. There is no registry and no network; `install` takes a local file.
- Transitive dependencies between packs. The set is flat.
- Signature verification. See spec 0055's non-goals.
