# Writing documentation

The core premise: **signatures come from the compiler; prose attaches
to items.** You never write a signature the compiler can produce — you
write comments, and udg extracts them alongside the declarations they
belong to.

## Comment syntax

All the usual spellings work: `/** … */`, `/*! … */`, `///`, `//!`.
Tags may be `@tag` or `\tag`, and Sphinx-style field lists are
accepted alongside Doxygen tags:

```cpp
/**
* Sign a message.
*
* Prose here is **Markdown**: `code`, lists, fenced blocks with
* syntax highlighting, and links all render.
*
* @param rng the random number generator to use
* @param padding the padding scheme, e.g. "PSS(SHA-256)"
* @return the signature bytes
* @throws Invalid_Argument if the padding scheme is unknown
* @see PK_Verifier
* @since 2.0
* @warning Reusing a nonce destroys the key.
*/
std::vector<uint8_t> sign(RandomNumberGenerator& rng, ...);
```

Recognized tags: `@brief`/`@short`, `@param` (with optional `[in]`,
`[out]`, `[in,out]`), `@return`/`@returns`/`@result`, `@throws` and
synonyms, `@warning`, `@note`/`@remark`, `@see`/`@sa`, `@since`,
`@deprecated`. Sphinx equivalents: `:param x:`, `:param type x:` (the
last word is the name), `:returns:`, `:raises Type:`.

The first paragraph is the item's summary (shown in listings and
search); an explicit `@brief` overrides it and still renders on the
page. Unknown tags stay in the body as prose — `user@example.com`
never becomes a tag.

**Trailing comments** document the *preceding* declaration and are the
way to document enum constants in place:

```cpp
enum class ErrorType {
   Unknown = 0,     ///< Not otherwise classified.
   InvalidArgument, /**< A parameter was malformed. */
};
```

## What gets extracted for you

- **Deleted functions** (`= delete`) are not API and never appear.
  `= default` and pure-virtual `= 0` members document normally.
- **Unnamed-enum constants** (`enum { LIMIT = 64 };`, the
  pre-`constexpr` idiom) hoist into the enclosing scope and render as
  a constants table on the class page.
- **Doc inheritance**: an override with no docs of its own takes the
  closest documented declaration in its override chain. Document the
  base-class contract once; every implementation shows it. (Parameter
  lints stand down for inherited docs, since the base may spell its
  parameters differently.)
- **`#define` macros** in documented headers are items. Because the
  compiler has no comment attachment for macros, docs must be
  *directly adjacent*: a `///` run just above the `#define`, or a
  trailing `///<` on its line. Function-like macros take `@param`.
  Header guards, feature flags (undocumented empty defines),
  underscore-prefixed names, and attribute-rule macros are never
  documented.
- **Attribute macros** (`BOTAN_PUBLIC_API(2,0)` and friends) become
  since/deprecated badges via [config rules](configuration.md#cpp-and-c),
  read from spelled tokens so they survive configurations where the
  macro expands to nothing.

For Python: docstrings, with the same field lists. Module-level
assignments take Sphinx `#:` doc comments (trailing or on the line
above) — that is also what makes a non-ALL-CAPS alias like
`MPILike = Union[...]` documentable at all. Class attributes
(`KEY_COMPROMISE = 1  #: the key leaked`) render as value tables.

## Autolinking

- Type names in signatures link to their pages — only *type
  positions*; a parameter merely named like a class stays plain text.
- `` `Name` `` code spans in prose link when the name resolves
  uniquely. Members resolve in the enclosing scope first, so
  `` `sign` `` inside `PK_Signer`'s docs finds `PK_Signer::sign`.
- Ambiguous names render as plain code rather than guessing. Qualify
  (`` `PK_Signer::sign` ``) to disambiguate; any depth of
  qualification works.
- `@see` references are *checked*: a reference that doesn't resolve is
  a `broken-ref` finding, an ambiguous one an `ambiguous-ref` finding.

## Conventions worth adopting

- Keep non-public API out of public docs with `hide` globs
  (`*::detail`, `*::_*`) instead of doc-comment tricks.
- Document virtual contracts on the base class; let inheritance fill
  the derived pages.
- Give every parameter a `@param` line. `udg check
  --undocumented-params` lists the gaps, and `require_param_docs`
  makes completeness a gate.
- One-line `///<` docs on enum constants and flag macros are cheap and
  render everywhere the value appears.
