//! `udg.toml` parsing and resolution into frontend configs.

use std::path::{Path, PathBuf};

use crate::frontend::clang::{AttributeRule, CppConfig};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UdgConfig {
    pub project: ProjectConfig,
    #[serde(default)]
    pub cpp: Option<CppSection>,
    /// A C API surface, extracted with the clang frontend but presented
    /// as its own tree. Same shape as `[cpp]`; set `c_mode = true` to
    /// parse as C (default here is C++ parsing with C presentation,
    /// which fits C APIs exported from C++ headers like Botan's ffi.h).
    #[serde(default)]
    pub c: Option<CppSection>,
    #[serde(default)]
    pub python: Option<PythonSection>,
    #[serde(default)]
    pub modules: Option<ModulesSection>,
    #[serde(default)]
    pub bindings: Option<BindingsSection>,
    #[serde(default)]
    pub guide: Option<GuideSection>,
    #[serde(default)]
    pub check: Option<CheckSection>,
    #[serde(default)]
    pub output: Option<OutputSection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSection {
    /// Minimum doc coverage percent per tree prefix (e.g. cpp = 40.0).
    #[serde(default)]
    pub min_coverage: std::collections::HashMap<String, f64>,
    /// Fail on ANY undocumented public item: `true` for every tree, or
    /// a list of tree prefixes (["py"]) to lock trees one at a time.
    #[serde(default)]
    pub require_docs: Option<RequireDocs>,
    /// Fail when a documented function documents none of its
    /// parameters (`@param` / `:param x:`). Same shape as require_docs.
    #[serde(default)]
    pub require_param_docs: Option<RequireDocs>,
    /// Coverage ratchet file: recorded per-tree coverage may never
    /// decrease. Update with `udg check --update-ratchet`.
    #[serde(default)]
    pub ratchet: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingsSection {
    /// TOML file of `[[bind]]` entries (see udg-link).
    pub map_file: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideSection {
    /// Directories of Markdown and/or reStructuredText pages.
    pub src: Vec<PathBuf>,
    /// Language for bare rst literal blocks (Sphinx highlight_language).
    #[serde(default)]
    pub default_code_language: Option<String>,
    /// File stems to skip (Sphinx scaffolding like `contents`).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Filesystem boundary for `literalinclude` (defaults to the first
    /// guide dir). Docs that embed source examples set this to the
    /// project root.
    #[serde(default)]
    pub include_root: Option<PathBuf>,
    #[serde(default = "default_guide_title")]
    pub title: String,
}

fn default_guide_title() -> String {
    "Handbook".into()
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RequireDocs {
    All(bool),
    Trees(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonSection {
    /// Python module files (module name = file stem).
    pub modules: Vec<PathBuf>,
    /// Glob patterns over qualified names for items that are not public
    /// API by project convention; matching items (and everything inside
    /// them) are excluded from docs, coverage, and checks.
    #[serde(default)]
    pub hide: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulesSection {
    #[serde(default)]
    pub cpp: Option<ModuleMapRef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleMapRef {
    pub map: MapSource,
}

/// Where a module map comes from: a command printing the JSON schema to
/// stdout (run with the config directory as cwd), or a static file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapSource {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSection {
    pub dir: PathBuf,
    /// Project stylesheet loaded after the built-in one; override the
    /// CSS custom properties here to re-theme without forking.
    #[serde(default)]
    pub extra_css: Option<PathBuf>,
    /// Logo image shown in the top bar next to the project name.
    #[serde(default)]
    pub logo: Option<PathBuf>,
    #[serde(default)]
    pub favicon: Option<PathBuf>,
    /// Footer line shown on every page (plain text).
    #[serde(default)]
    pub footer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
    /// Root namespace stripped from display names and page URLs.
    #[serde(default)]
    pub root_namespace: Option<String>,
    /// Directory that relative config paths resolve against, itself
    /// relative to the config file (default: the config's directory).
    /// Lets a config live at src/configs/udg/udg.toml while its paths
    /// stay checkout-relative: root = "../../..".
    #[serde(default)]
    pub root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CppSection {
    #[serde(default = "default_std")]
    pub std: String,
    #[serde(default)]
    pub c_mode: bool,
    #[serde(default)]
    pub include_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Header paths or glob patterns, relative to the config file.
    /// May be empty when `[modules.cpp]` supplies headers instead.
    #[serde(default)]
    pub headers: Vec<String>,
    #[serde(default)]
    pub attributes: Vec<AttrRuleSection>,
    /// How these headers are included, for "Defined in" display:
    /// `include_prefix = "botan"` renders <botan/pubkey.h>.
    #[serde(default)]
    pub include_prefix: Option<String>,
    /// Take include paths, defines, and language flags from a compile
    /// database (the CMake/ninja lingua franca) instead of hand-listing
    /// them. One representative entry supplies the flags.
    #[serde(default)]
    pub compile_commands: Option<PathBuf>,
    /// Substring selecting which database entry's flags to use
    /// (default: the first entry).
    #[serde(default)]
    pub compile_commands_source: Option<String>,
    /// Glob patterns over qualified names for items that are not public
    /// API by project convention — `"*::detail"` for detail namespaces,
    /// `"*::_*"` for underscore-prefixed members. Matching items (and
    /// everything inside them) are excluded from docs, coverage, and
    /// checks.
    #[serde(default)]
    pub hide: Vec<String>,
    /// Proceed despite clang error diagnostics. Off by default: errors
    /// mean an incomplete AST, and coverage over a partial AST lies.
    #[serde(default)]
    pub allow_parse_errors: bool,
    /// Glob patterns removing files matched by `headers` (build shims,
    /// config headers that refuse late inclusion, ...).
    #[serde(default)]
    pub exclude_headers: Vec<String>,
    /// Shipped-but-not-public headers (vendored specs like PKCS#11):
    /// xrefs to their names resolve instead of counting as rot, but
    /// nothing in them is documented or counted.
    #[serde(default)]
    pub external_headers: Vec<String>,
}

fn default_std() -> String {
    "c++20".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttrRuleSection {
    #[serde(rename = "match")]
    pub match_: AttrMatch,
    pub effect: AttrEffect,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttrMatch {
    #[serde(rename = "macro")]
    pub macro_: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttrEffect {
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub deprecated: Option<String>,
}

pub fn load(path: &Path) -> Result<(UdgConfig, PathBuf)> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config {}", path.display()))?;
    let cfg: UdgConfig =
        toml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))?;
    let base = path
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))?
        .parent()
        .expect("config file has a parent directory")
        .to_path_buf();
    // A config nested in the tree (src/configs/udg/udg.toml) sets
    // [project] root = "../../.." so its paths stay checkout-relative.
    let base = match &cfg.project.root {
        Some(r) => {
            let joined = if r.is_absolute() {
                r.clone()
            } else {
                base.join(r)
            };
            joined.canonicalize().with_context(|| {
                format!(
                    "[project] root `{}` does not resolve (from {})",
                    r.display(),
                    path.display()
                )
            })?
        }
        None => base,
    };
    Ok((cfg, base))
}

/// Split a POSIX-ish shell command line into tokens (quotes and
/// backslash escapes; no expansion).
fn shell_split(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Extract the parse-relevant flags from one compile-database command:
/// includes, defines, language/target flags. Relative paths resolve
/// against the entry's working directory. Warning flags, optimization,
/// and output options are dropped.
fn extract_parse_flags(tokens: &[String], dir: &Path) -> Vec<String> {
    let abs = |p: &str| -> String {
        if Path::new(p).is_absolute() {
            p.to_owned()
        } else {
            dir.join(p).display().to_string()
        }
    };
    // Flags whose (joined or separate) argument is a path.
    const PATH_FLAGS: &[&str] = &[
        "-I",
        "-isystem",
        "-iquote",
        "-idirafter",
        "-include",
        "-isysroot",
    ];
    const KEEP_PREFIX: &[&str] = &[
        "-D",
        "-U",
        "-std=",
        "-stdlib=",
        "-f",
        "-m",
        "--target=",
        "--sysroot=",
        "-nostdinc",
        "-pthread",
        "-x",
    ];

    let mut out = Vec::new();
    let mut i = 1; // skip the compiler executable
    while i < tokens.len() {
        let t = tokens[i].as_str();
        if let Some(flag) = PATH_FLAGS.iter().find(|f| t == **f) {
            if let Some(arg) = tokens.get(i + 1) {
                out.push((*flag).to_owned());
                out.push(abs(arg));
                i += 2;
                continue;
            }
        } else if let Some(flag) = PATH_FLAGS
            .iter()
            .find(|f| t.starts_with(**f) && t.len() > f.len())
        {
            out.push(format!("{flag}{}", abs(&t[flag.len()..])));
        } else if t == "-o" || t == "-c" || t == "-x" {
            // -x keeps its argument (language), -o/-c drop theirs/none.
            if t == "-x" {
                if let Some(arg) = tokens.get(i + 1) {
                    out.push("-x".into());
                    out.push(arg.clone());
                    i += 2;
                    continue;
                }
            } else if t == "-o" {
                i += 2;
                continue;
            }
        } else if KEEP_PREFIX.iter().any(|p| t.starts_with(p)) {
            // OpenMP matters for codegen, not API extraction, and pulls
            // in omp.h that libclang often cannot find.
            if !t.starts_with("-fopenmp") {
                out.push(t.to_owned());
            }
        }
        i += 1;
    }
    out
}

/// Flags from the configured compile database entry.
fn compile_commands_flags(path: &Path, selector: Option<&str>) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Entry {
        directory: String,
        file: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        arguments: Option<Vec<String>>,
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read compile database {}", path.display()))?;
    let entries: Vec<Entry> = serde_json::from_str(&text)
        .with_context(|| format!("invalid compile database {}", path.display()))?;
    let entry = entries
        .iter()
        .find(|e| selector.is_none_or(|s| e.file.contains(s)))
        .with_context(|| match selector {
            Some(s) => format!("no compile database entry matches `{s}`"),
            None => "compile database is empty".into(),
        })?;
    let tokens = match (&entry.arguments, &entry.command) {
        (Some(args), _) => args.clone(),
        (None, Some(cmd)) => shell_split(cmd),
        (None, None) => bail!("compile database entry has neither command nor arguments"),
    };
    Ok(extract_parse_flags(&tokens, Path::new(&entry.directory)))
}

/// Resolve a `[cpp]`/`[c]` section into a frontend config: globs
/// expanded, paths made absolute relative to the config file's directory.
pub fn resolve_cpp(
    section: &CppSection,
    base: &Path,
    language: Option<crate::ir::Language>,
) -> Result<CppConfig> {
    let mut headers = Vec::new();
    for pattern in &section.headers {
        let absolute = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            base.join(pattern).display().to_string()
        };
        if pattern.contains(['*', '?', '[']) {
            let before = headers.len();
            for entry in
                glob::glob(&absolute).with_context(|| format!("bad glob pattern {pattern}"))?
            {
                headers.push(entry?);
            }
            if headers.len() == before {
                bail!("header pattern matched no files: {pattern}");
            }
        } else {
            let p = PathBuf::from(&absolute);
            if !p.exists() {
                bail!("header not found: {pattern} (resolved to {absolute})");
            }
            headers.push(p);
        }
    }
    let mut external_headers = Vec::new();
    for pattern in &section.external_headers {
        let absolute = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            base.join(pattern).display().to_string()
        };
        if pattern.contains(['*', '?', '[']) {
            for entry in
                glob::glob(&absolute).with_context(|| format!("bad glob pattern {pattern}"))?
            {
                external_headers.push(entry?);
            }
        } else {
            external_headers.push(PathBuf::from(absolute));
        }
    }

    if !section.exclude_headers.is_empty() {
        let patterns: Vec<glob::Pattern> = section
            .exclude_headers
            .iter()
            .map(|g| {
                glob::Pattern::new(g)
                    .map_err(|e| anyhow::anyhow!("exclude_headers: invalid pattern `{g}`: {e}"))
            })
            .collect::<Result<_>>()?;
        headers.retain(|h| {
            let s = h.display().to_string();
            !patterns.iter().any(|p| p.matches(&s))
        });
    }
    headers.sort();
    headers.dedup();

    Ok(CppConfig {
        std: section.std.clone(),
        c_mode: section.c_mode,
        language,
        include_dirs: section
            .include_dirs
            .iter()
            .map(|d| {
                if d.is_absolute() {
                    d.clone()
                } else {
                    base.join(d)
                }
            })
            .collect(),
        external_headers,
        defines: section.defines.clone(),
        extra_args: {
            let mut args = match &section.compile_commands {
                Some(cc) => {
                    let cc = if cc.is_absolute() {
                        cc.clone()
                    } else {
                        base.join(cc)
                    };
                    compile_commands_flags(&cc, section.compile_commands_source.as_deref())?
                }
                None => Vec::new(),
            };
            args.extend(section.extra_args.iter().cloned());
            args
        },
        headers,
        attributes: section
            .attributes
            .iter()
            .map(|a| AttributeRule {
                macro_name: a.match_.macro_.clone(),
                arg_names: a.match_.args.clone(),
                since: a.effect.since.clone(),
                deprecated: a.effect.deprecated.clone(),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_db_flag_extraction() {
        let tokens = shell_split(
            r#"g++ -fPIC -m64 -pthread -std=c++20 -D_REENTRANT -O3 -DFOO=1 -Wall -Wextra -Ibuild/include/public -isystem /usr/inc -I "sp dir" -c src/x.cpp -o x.o"#,
        );
        let flags = extract_parse_flags(&tokens, Path::new("/work"));
        assert!(flags.contains(&"-std=c++20".to_owned()));
        assert!(flags.contains(&"-DFOO=1".to_owned()));
        assert!(flags.contains(&"-I/work/build/include/public".to_owned()));
        assert!(flags.contains(&"-isystem".to_owned()) && flags.contains(&"/usr/inc".to_owned()));
        let pos = flags
            .iter()
            .position(|f| f == "-I")
            .expect("separate -I kept");
        assert_eq!(flags[pos + 1], "/work/sp dir");
        assert!(
            !flags
                .iter()
                .any(|f| f.starts_with("-W") || f == "-O3" || f == "-c" || f == "-o")
        );
        assert!(
            !flags
                .iter()
                .any(|f| f.contains("x.cpp") || f.contains("x.o") || f == "g++")
        );
    }
}
