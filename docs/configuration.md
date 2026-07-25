# Configuration reference

One TOML file drives everything. Unknown keys are errors (typos never
silently disable a feature), and all relative paths resolve against
the config file's directory unless `[project] root` says otherwise.

## [project]

```toml
[project]
name = "Botan"
root_namespace = "Botan"    # stripped from display names and URLs
root = "../../.."           # optional: base for all relative paths
```

`root` exists for configs that live inside the tree (Botan keeps its
at `src/configs/udg/udg.toml`): set it to the checkout root and every
other path in the file stays checkout-relative. The module-map command
also runs with `root` as its working directory.

## [cpp] and [c]

Both sections have the same shape; `[c]` produces a separate tree
presented as a C API. Botan's ffi.h is the model case: parsed as C++
(it uses `[[deprecated]]`), presented as C. Set `c_mode = true` to
actually parse as C.

```toml
[cpp]
std = "c++20"
headers = ["include/mylib/*.h"]      # globs; may be empty if a module
                                     # map supplies headers instead
exclude_headers = ["*/dll.h"]        # drop matched files (build shims)
include_dirs = ["include"]
defines = ["MYLIB_NO_DEPRECATION_WARNINGS"]
extra_args = ["-stdlib=libc++"]
include_prefix = "mylib"             # "Defined in <mylib/foo.h>"
```

**Compile databases.** Instead of hand-listing include paths:

```toml
compile_commands = "build/compile_commands.json"
compile_commands_source = "geometry/PointCloud.cpp"   # substring picks
                                                      # the entry
```

One representative entry supplies flags for the whole parse. udg keeps
include/define/language flags and drops warning, optimization, and
output flags. Remember CMake only writes the database with
`-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, and ExternalProject include
trees exist only after their targets build.

**Attribute macros.** Project macros carrying API metadata are read
from the *spelled* tokens, so they work even in configurations where
the macro expands to nothing:

```toml
[[cpp.attributes]]
match = { macro = "BOTAN_PUBLIC_API", args = ["major", "minor"] }
effect = { since = "{major}.{minor}" }

[[cpp.attributes]]
match = { macro = "BOTAN_DEPRECATED", args = ["msg"] }
effect = { deprecated = "{msg}" }
```

Rule-named macros are treated as markup: they never become documented
macro items, though references to them still resolve.

**Hiding non-public API.** Glob patterns over qualified names; a match
prunes the whole subtree from docs, coverage, search, and checks:

```toml
hide = ["*::detail", "*::_*"]
```

`*` crosses `::`, so `*::detail` catches `NS::detail` at any depth.
Encode your project's convention — nothing is hardcoded.

**External reference headers.** Vendored spec headers (a bundled
PKCS#11 header, say) are shipped but not your API. Naming them makes
cross-references to their declarations and macros resolve instead of
counting as rot, without documenting or counting anything in them:

```toml
external_headers = ["build/include/external/*.h"]
```

**Parse errors.** Error-severity clang diagnostics abort extraction —
coverage over a partial AST lies. `allow_parse_errors = true` opts
out; prefer fixing the include or excluding the module.

## [python]

```toml
[python]
modules = ["src/python/mylib.py"]   # module name = file stem
hide = ["SomeClass.*"]              # same glob semantics
```

Extraction is fully static — the module is never imported, so doc
builds need no Python environment and no built native library.
`__all__`, when present, decides module-level inclusion. Two files
with the same stem are an error (module identity is the stem).

## [modules]

Without a map, everything lands in one flat "api" module — fine to
start. With one, the sidebar and URL structure follow your project's
own module metadata:

```toml
[modules.cpp]
map = { command = "python3 scripts/module_map.py ." }
# or: map = { file = "modules.json" }
```

The command prints JSON to stdout (run from `[project] root`):

```json
{ "modules": [
  { "id": "pubkey", "name": "Public Key Algorithms",
    "brief": "…", "headers": ["src/lib/pubkey/pubkey.h"] },
  { "id": "pubkey/ec_group", "name": "EC Group", "parent": "pubkey",
    "headers": ["src/lib/pubkey/ec_group/ec_group.h"] }
] }
```

Ids become URL path segments (validated: filename-safe characters, no
`..`); duplicate ids, unknown parents, and parent cycles are errors.
Items are assigned to modules by source file; extracted items whose
header appears in no module surface as `unassigned` check findings.

## [bindings]

Cross-language "Also available in" links, from a small map:

```toml
[bindings]
map_file = "scripts/bindings.toml"
```

```toml
# bindings.toml
[[bind]]
c_prefix = "botan_hash_"      # a whole family of C functions
cpp = "HashFunction"
py = "HashFunction"
```

Entries that stop matching anything become `binding-map` check
findings — the map cannot rot silently.

## [guide]

Prose pages (Markdown and/or reStructuredText), rendered with API
autolinks and listed in their own nav tree:

```toml
[guide]
src = ["doc", "doc/api_ref"]
exclude = ["contents", "index"]      # Sphinx scaffolding, by stem
default_code_language = "cpp"        # Sphinx highlight_language
title = "Handbook"
include_root = "."                   # literalinclude boundary
```

`include_root` defaults to the config's directory; `literalinclude`
may not read outside it. See [Prose and guides](prose-and-guides.md).

## [check]

```toml
[check]
min_coverage = { cpp = 35.0, c = 45.0 }       # hand-set floors
ratchet = "scripts/coverage.toml"              # machine-written high-water mark
require_docs = ["c", "py"]                     # or true for all trees
require_param_docs = ["c"]
```

See [Checks](checks.md) for how the three gates layer. Gate lists
naming a tree that doesn't exist are errors — a misspelled tree name
must not silently disable enforcement. Tree prefixes are `cpp`, `c`,
and `py`.

## [output]

```toml
[output]
dir = "build/docs"
extra_css = "themes/rtd.css"    # loaded after the builtin stylesheet
logo = "doc/logo.png"
favicon = "doc/favicon.ico"
footer = "MyLib is released under the MIT license."
```

Theming is CSS custom properties plus selector overrides in one
stylesheet — see `themes/` in the udg repository (also in the release
tarballs) for two complete examples, including a Read the Docs look.
Copy one next to your config; `cargo install` ships only the binary. Templates are deliberately
not overridable.
