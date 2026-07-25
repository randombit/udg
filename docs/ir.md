# The udg IR JSON

`udg extract --output out.json` dumps the intermediate representation
that all frontends emit and all later stages consume. It is intended to
become a stable, versioned interface for external tooling (API diffing,
coverage dashboards), like rustdoc's JSON output.

Stability: **not yet stable.** `ir_version` is `0`; the shape may change
without notice until it is bumped to `1`. Fields with `None`/empty
values are omitted from the JSON.

Top level:

```json
{
  "ir_version": 0,
  "generator": "udg 0.1.0",
  "project": "Botan",
  "trees": [ { "title", "prefix", "root_namespace"?, "modules": [...], "items": [...] } ]
}
```

`Module`: `{ id, name, brief?, parent?, headers: [path] }` — the
navigation grouping, supplied by the project (module-map hook), never
inferred.

`Item` (recursive via `children`):

- `id` — stable, path-derived: `cpp:Botan::PK_Signer::sign_message`.
  Overloads currently share an id.
- `lang` — `cpp` | `c` | `python`; `kind` — `class`, `method`, `enum`, …
- `name`, `qualified_name`
- `signature` — structured, never a pre-rendered string: `return_type?`,
  `params: [{name?, type, default?}]`, `is_const`/`is_virtual`/… flags,
  `template_params?`
- `docs` — parsed comment: `summary?`, `body?` (Markdown), `params`,
  `returns?`, `throws`, `warnings`, `notes`, `see_also`, `since?`,
  `deprecated?`
- `since?` / `deprecated?` — from mapped project attributes
  (e.g. `BOTAN_PUBLIC_API(2,0)`) or doc tags
- `access?`, `bases`, `type?` (enum underlying / alias target /
  variable type), `value?` (enumerators), `source?` (`file`,
  `line_start`, `line_end`), `module?`
- `bindings` — ids of corresponding items in other language trees,
  filled by the linker from the binding map
