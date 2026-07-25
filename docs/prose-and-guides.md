# Prose and guides

`[guide]` renders a directory of Markdown and reStructuredText pages
into a handbook alongside the API reference — with API autolinks, its
own nav tree, and the same lints as everything else. Sphinx-era rst
works unchanged; that is the point.

## What the rst dialect covers

Sections and titles, paragraphs, bullet/enumerated lists, literal
blocks (including bare `::` blocks, highlighted per
`default_code_language`), block quotes, line blocks, grid *and* simple
tables, `list-table`, admonitions (`note`, `warning`, and friends,
rendered with title bars), `versionadded`/`versionchanged`/
`deprecated`, `code-block` with syntax highlighting, `rubric`,
`highlight`, `toctree`, `only`, `literalinclude`, inline markup
(emphasis, strong, ``literals``, roles), `:rfc:` links, and hyperlink
targets.

The deliberate cap: the Sphinx **extension API** is never supported.
Unknown directives render their body and produce a warning — gaps are
loud, never silently mis-rendered. Those warnings surface in
`udg check` as `rst-dialect` findings, so a page that stops rendering
fully cannot slip by.

## Navigation

The first `contents`/`index` file's `toctree` drives nav order;
remaining pages sort alphabetically. Entries that match no page warn —
a chapter cannot silently vanish from the sidebar. Scaffolding pages
themselves are usually excluded by stem (`exclude = ["contents",
"index"]`); excluded pages still contribute their `toctree` ordering.

## Cross-references

- `:doc:`building`` links a guide page by stem, titled with the page's
  own title.
- `:ref:`handshake_complete`` resolves against explicit `.. _label:`
  targets **and** Sphinx autosectionlabel-style names
  (`docname:section title`) collected across the whole guide.
- Code-ish roles (`:cpp:func:`, `:py:meth:`, `:c:macro:`, …) render as
  autolinked code spans. A role naming something that exists at *no*
  qualification depth is a `rst-dialect` finding — a declared API
  reference that resolves to nothing is rot. External names (spec
  constants from a vendored header) resolve via
  [`external_headers`](configuration.md#cpp-and-c); truly external
  things (a Boost class) belong in ``literal`` markup, which is never
  checked.
- Unresolvable `:doc:`/`:ref:` targets warn instead of degrading to
  plain text silently.

## Embedding source files

```rst
.. literalinclude:: /../src/examples/aes.cpp
   :language: cpp
```

Sphinx semantics: `/`-prefixed paths resolve from the doc source root,
relative paths from the including file. Reads are confined to
`[guide] include_root` (default: the config's directory) so
documentation sources cannot publish arbitrary files; projects that
embed examples from across the tree set it to the checkout root.

## `.. only::`

udg is the HTML builder, so `only:: html` bodies (and conjunctions
like `html and website`) render; other builders' bodies (`latex`,
`man`) are excluded intentionally. Negations are conservatively
skipped.

## Transclusion: one source of truth

Hand-written signatures drift. Instead of restating a declaration in
prose, embed the extracted one:

**Python — standard Sphinx autodoc, resolved against the extracted IR
(the module is never imported):**

```rst
.. py:module:: botan3

.. autofunction:: version_major

.. autoclass:: HashFunction
   :members:
```

Bare names qualify against the `py:module` context; `:members:`
appends public documented members; unknown options warn.

**C++/C — the language-agnostic form:**

```rst
.. udg:member:: X509_Certificate::subject_public_key

   Narrative prose for this member continues here, rendered beneath
   the embedded signature and parameter docs.
```

Hand-written `.. cpp:function::` directives still render (styled like
reference members) but are checked by the guide-drift lint: a
signature that matches no overload, or documents a name that no longer
exists, is a finding. Migrate them to transclusion as convenient — see
[Migration](migration.md).
