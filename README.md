# udg

A fast, easy to use, and non-fiddly documentation generator and analyzer for
multilanguage projects. The rustdoc experience is great and this tool aims to
replicate that experience for C/C++/Python APIs.

It parses your API surface (using libClang for C/C++ and ruff's parser for Python),
merges with prose documentation in Markdown or reStructuredText, and generates a
static site.

You can also use it to understand and manage your documentation coverage,
including features to help gate CI. `udg check` reports documented parameters
that don't exist, undocumented parameters, broken `@see` references, and
hand-written signatures in prose that no longer match the code, and it can
fail the build when coverage drops.

```console
$ udg build
udg: [cpp] doc coverage 2448/6144 (40%)
udg: [c]   doc coverage 245/475 (52%)
udg: [python] doc coverage 197/343 (57%)
udg: rendered 1956 pages (8555 search entries) to build/docs
```

## Why

The current meta for documenting a C++ library with Python bindings is Doxygen
plus Sphinx plus Breathe to bridge them. All three are slow, Doxygen misparses
various newer C++ constructs, if you make a syntax error in rst Sphinx silently
drops sections, if declarations in the docs don't match the code nothing tells
you, and on and on. Overall, it's not a pleasant experience.

## Installation

udg is a single binary that loads libclang at runtime. Install libclang from
your package manager first:

```console
$ sudo apt install libclang-dev     # Debian/Ubuntu
$ sudo pacman -S clang              # Arch
$ brew install llvm                 # macOS
```

Then either download a prebuilt tarball from the
[GitHub releases](https://github.com/randombit/udg/releases) page, or build
from source with a Rust toolchain (1.96 or newer):

```console
$ cargo install udg
```

If libclang lives somewhere unusual, set `LIBCLANG_PATH` to its directory.
Binaries built from a checkout link libclang at build time instead; pass
`--features dlopen-libclang` to get the runtime-loading behavior of the
release builds.

The example stylesheets in `themes/` are included in the repository and the
release tarballs but not in `cargo install`; copy one next to your config to
use it.

## Quick start

A minimal project needs only a few lines of `udg.toml`, for example

```toml
[project]
name = "LibraryX"
root_namespace = "LibX"

[cpp]
std = "c++17"
include_prefix = "libx"
include_dirs = ["."]
headers = ["libx/*.h"]

[output]
dir = "build/docs"
```

```console
$ udg build            # extract + render the site
$ udg serve            # build and serve locally
$ udg check            # doc linting and coverage report
```

## Documentation

- [Getting started](docs/getting-started.md) — from a checkout to a browsable site
- [Configuration](docs/configuration.md) — every `udg.toml` option
- [Writing documentation](docs/writing-docs.md) — the comment conventions udg understands
- [Prose and guides](docs/prose-and-guides.md) — Markdown/rst handbook pages and transclusion
- [Checks](docs/checks.md) — the lints and coverage gates
- [Migrating from Sphinx + Doxygen](docs/migration.md) — the incremental path for an existing project
- [JSON IR](docs/ir.md) — the `udg extract` output format for external tooling

## License

MIT
