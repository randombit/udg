# Checks: docs as a CI gate

`udg check` re-extracts everything and runs every lint. It exits
non-zero when **any** finding exists or a coverage gate fails — it is
a gate, not advisory. Every finding is `file:line: [lint] message`
with paths canonicalized to the real source location you would edit.

## The lints

**`param-mismatch`** — a documented parameter that doesn't exist:

```
src/lib/pubkey/pubkey.h:214: [param-mismatch] @param `randomness` does
not match any parameter of `Botan::PK_Signer::sign`
```

Usually a rename that didn't reach the comment. Fires whenever an item
documents at least one parameter.

**`param-undocumented`** — a parameter without docs, on a function
that documents its others (always on) or documents anything at all
(with `require_param_docs`).

**`broken-ref` / `ambiguous-ref`** — `@see` references that resolve to
nothing / to several things. Qualify ambiguous ones.

**`guide-drift`** — hand-written signature directives in prose checked
against the extracted API: names that no longer exist, and argument
counts that fit no overload. This lint found seventy-plus stale
signatures in Botan's handbook on its first run, including a
documented API that had been removed. Directives are matched within
their own language domain (C and C++ interchange; Python does not).

**`rst-dialect`** — everything the guide renderer warns about:
unsupported directives, unresolvable transclusions, `:ref:`/`:doc:`
targets that match nothing, xref roles naming unknown symbols, dead
`toctree` entries.

**`binding-map`** — cross-language binding entries that no longer
match any symbol.

**`unassigned`** — items extracted from a header that belongs to no
module: they'd be counted but never rendered. Add the header to the
module map or drop it.

**`undocumented`** — public items with no docs at all. Off by default
(that's coverage's job); enabled by `--undocumented` or
`require_docs`.

## The three coverage gates

They layer, strictest last:

1. **`min_coverage`** — a hand-set floor per tree, deliberately below
   current reality. Survives everything; catches catastrophe.
2. **`ratchet`** — a machine-written high-water mark: coverage may
   never drop below the recorded value. Never edit the file by hand;
   after documentation lands, record progress:

   ```console
   $ udg check --update-ratchet
   ```

   A configured ratchet file that is missing or unparseable is an
   error, not a silent skip (`--update-ratchet` bootstraps it).
3. **`require_docs` / `require_param_docs`** — for trees that reached
   zero: any future undocumented item (or parameter) fails the build.
   The end state, flipped one tree at a time:

   ```toml
   [check]
   require_docs = ["c", "py"]
   ```

## The improvement loop

```console
$ udg check --undocumented          # the worklist
$ $EDITOR ...                       # document, base classes first —
                                    # doc inheritance multiplies payoff
$ udg check --update-ratchet        # lock in the gain
```

When a tree's `--undocumented` list hits zero, add it to
`require_docs` and it can never regress. Botan's C and Python trees
reached 100% this way within weeks of adopting the loop.

## CI

```yaml
- name: Documentation
  run: udg check
```

That's the whole integration: exit code carries the verdict, findings
print as annotations-friendly `file:line:` lines. Extraction failures
(clang errors, missing ratchet, invalid module map, gates naming
unknown trees) are hard errors — every fail-open path has been
deliberately closed.
