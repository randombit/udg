//! udg - a documentation generator for C++/C/Python libraries.
//!
//! The pipeline is a strict sequence of stages, one module each:
//!
//! - [`ir`] — the language-neutral item model every stage speaks
//! - [`comments`] — doc-comment parser (Doxygen tag subset + Markdown)
//! - [`frontend`] — extractors: libclang for C++/C, ruff for Python
//! - [`link`] — symbol index, cross-language binding map, alias dedup
//! - [`check`] — lints and coverage gates (`udg check`)
//! - [`render`] — static-site renderer (`udg build`)
//! - [`config`] / [`modules`] — `udg.toml` and the module-map hook
//! - [`pipeline`] — the subcommand bodies that string the stages together
//! - [`serve`] — the std-only static server behind `udg serve`
//!
//! The binary in `main.rs` is only the clap surface over [`pipeline`].
//! The JSON emitted by `udg extract` (see `docs/ir.md`) is the intended
//! interface for external frontends and tooling; this library API is
//! not stable.

pub mod check;
pub mod comments;
pub mod config;
pub mod frontend;
pub mod ir;
pub mod link;
pub mod modules;
pub mod pipeline;
pub mod render;
pub mod serve;
