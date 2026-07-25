# Migrating from Sphinx + Doxygen

udg is designed so an existing Sphinx/Doxygen project migrates
incrementally: point it at what you already have, let the warnings
tell you what needs attention, and retire the old pipeline when the
output is better than what it replaced. Nothing requires a flag-day
rewrite of comments or prose.

## Step 1: extract the API

Start with just `[project]`, `[cpp]` (or `compile_commands`), and
`[output]` — see [Getting started](getting-started.md). Get to **zero
clang errors**; udg refuses to build docs from a partial AST. The
usual fixes, in order: missing include dirs, a define the headers
expect, `exclude_headers` for build shims, and for CMake trees making
sure ExternalProject dependencies have actually unpacked
(`UDG_DEBUG_ARGS=1 udg build` prints the exact libclang argv when
you need it).

Your Doxygen comments work as they are: the tag set, trailing
`///<` forms, and attribute macros via config rules. A Doxygen-only
project is often done at this point — Crypto++ documented 7,000+ items
on a twelve-line config, first try.

## Step 2: point [guide] at your Sphinx tree

```toml
[guide]
src = ["doc", "doc/api_ref"]
exclude = ["contents", "index"]
default_code_language = "cpp"
include_root = "."
```

The rst prose dialect renders your pages as they are: sections,
tables, admonitions, `literalinclude`, `toctree` nav, `:ref:`/`:doc:`
cross-references including autosectionlabel style. What udg cannot
render it says so, loudly — each `rst-dialect` warning is one concrete
construct on one line, not a mystery. Expect a burst initially;
they divide into:

- Sphinx *extensions* (permanently out of scope) — rewrite those spots
  in the dialect.
- Genuinely broken references and stale content — the interesting
  pile. Fix upstream; your docs improve regardless of udg.
- Missing dialect features worth reporting.

## Step 3: read the drift report

If your handbook hand-writes signatures (`.. cpp:function::` blocks),
`udg check` compares every one against the real API. This is where
migration pays for itself immediately: renamed parameters, removed
functions still documented, signatures that quietly grew arguments.
Fix the genuine rot first; the directives themselves keep rendering.

## Step 4: replace duplication with transclusion

Where a handbook block duplicates a header comment, replace the
hand-written signature with an embed and keep the narrative:

```rst
.. udg:member:: X509_Certificate::subject_public_key

   The returned key is newly allocated; callers own it.
```

For Python, standard autodoc directives are the preferred spelling and
work as-is. Where the header comment is thin and the handbook prose is
good, move the prose *into the header* first — the reference page, the
handbook, and your IDE hover all improve at once.

## Step 5: turn on the gates

```toml
[check]
min_coverage = { cpp = 35.0 }
ratchet = "scripts/coverage.toml"
```

Grind `--undocumented` and `--undocumented-params` at whatever pace
suits; ratchet after each push. Trees that reach zero go into
`require_docs`. Wire `udg check` into CI early — even before coverage
is respectable, the lints stop *new* rot immediately.

## Step 6: retire the old pipeline

When the udg site covers what readers actually use: delete the Doxygen
config, the Breathe glue, and the Sphinx extension stack; keep your
rst as the guide source (or convert to Markdown at leisure — the
dialect makes this optional, not required). Point `[output] dir`
somewhere fresh rather than at the old Sphinx output directory — udg
refuses to replace a directory it didn't create, on purpose.

## Familiarity for readers

`themes/rtd.css` approximates the Read the Docs look closely enough
that regular users may not notice the engine changed — until they use
search, click a `[src]` link, or land on a cross-language binding
line. See `themes/README.md`.
