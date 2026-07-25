# Getting started

udg documents a C++/C/Python library as one static site: the API
reference is extracted from real ASTs (libclang for C and C++, static
parsing for Python), merged with your existing Markdown or
reStructuredText prose, and checked strictly enough to gate CI.

This page takes you from a checkout to a browsable site. The other
guides go deeper:

- [Configuration](configuration.md) — every `udg.toml` knob.
- [Writing documentation](writing-docs.md) — the comment conventions
  the extractor understands.
- [Prose and guides](prose-and-guides.md) — handbook pages, rst
  support, transclusion.
- [Checks](checks.md) — the lints and coverage gates.
- [Migrating from Sphinx + Doxygen](migration.md) — the incremental
  path for an existing project.

## 1. Install udg

udg needs a libclang shared library at runtime; install it from your
package manager (`libclang-dev` on Debian/Ubuntu, `clang` on Arch,
`llvm` via Homebrew on macOS). Then download a prebuilt tarball from
the [GitHub releases](https://github.com/randombit/udg/releases) page,
or build from source with a Rust toolchain (1.96 or newer):

```console
$ cargo install udg
```

or from a checkout:

```console
$ cargo build --release
```

If libclang lives somewhere unusual, point `LIBCLANG_PATH` at its
directory. Release binaries are built with the `dlopen-libclang`
feature and search for libclang at startup instead of linking it, so
any distro copy works; a checkout build links it at build time unless
you pass `--features dlopen-libclang`.

## 2. Write a minimal config

udg is driven by one TOML file, conventionally `udg.toml` at the
project root. The smallest useful config names the project and the
headers:

```toml
[project]
name = "MyLib"
root_namespace = "mylib"     # stripped from display names and URLs

[cpp]
std = "c++20"
headers = ["include/mylib/*.h"]
include_dirs = ["include"]

[output]
dir = "build/docs"
```

Every relative path resolves against the config file's directory (see
[`[project] root`](configuration.md#project) if the config lives in a
subdirectory). For CMake projects, replace the header bookkeeping with
`compile_commands = "build/compile_commands.json"` and udg inherits
the project's real include paths and defines.

## 3. Build the site

```console
$ udg build --open
udg: [cpp] parsing 42 headers (c++20, 0 modules)
udg: [cpp] doc coverage 310/512 (61%)
udg: rendered 214 pages (890 search entries) to build/docs
udg: opening build/docs/index.html
```

What just happened, step by step:

1. **Extraction.** udg composed one synthetic translation unit
   including every configured header and parsed it with libclang.
   Classes, functions, enums, macros, type aliases — everything public
   — became items in a language-neutral intermediate representation,
   with doc comments parsed into structured fields. clang *errors*
   abort the build (a partial AST would produce lying numbers); fix
   the include path or see
   [`allow_parse_errors`](configuration.md#cpp-and-c).
2. **Linking.** A symbol index over every item resolves type names in
   signatures, `` `Name` `` mentions in prose, `@see` references, and
   cross-language binding links.
3. **Rendering.** Static HTML: one page per class/enum, leaf pages for
   functions and constants, per-module source views with line anchors,
   a unified search index. The output works from `file://` — no web
   server required.

The output directory is replaced atomically on rebuild, and only
directories udg itself created (marked with a `.udg-site` file) are
ever replaced — pointing `[output] dir` at a directory with other
content in it is refused, not overwritten.

## 4. Browse

- The landing page lists each language tree. Modules form the sidebar;
  every class page shows constructors, methods, members, inherited
  members from its base chain, and non-member operators that take it.
- Press `s` to search. The index covers every item in every language.
- `[src]` links jump to the highlighted source view at the exact line.

## 5. Add the gates

```console
$ udg check
```

runs the documentation lints and exits non-zero on any finding — see
[Checks](checks.md). Start with nothing configured, look at what it
reports, then add coverage floors and ratchets as the numbers improve.

## Commands at a glance

```console
udg build   [-c udg.toml] [-o DIR] [--open]   # extract + render
udg check   [-c udg.toml]                     # lints + gates (CI)
udg check --undocumented                      # list undocumented items
udg check --undocumented-params               # list undocumented params
udg check --update-ratchet                    # record current coverage
udg serve   [-c udg.toml] [-p 8080]           # build + serve locally
udg extract [-c udg.toml] [-o FILE]           # dump the JSON IR
```
