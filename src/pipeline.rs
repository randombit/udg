//! Subcommand bodies: `extract`, `build`, `check`, `serve`. Each one
//! loads `udg.toml`, runs the frontends, and hands the IR to the
//! link/check/render stages; `main.rs` is only the clap surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config;
use crate::ir::{Item, ItemKind, Language, Module};
use crate::modules;
use crate::serve;
use anyhow::{Context, Result, bail};
use serde::Serialize;

pub fn check_cmd(
    config_path: &Path,
    update_ratchet: bool,
    undocumented: bool,
    undocumented_params: bool,
) -> Result<()> {
    let (cfg, base) = config::load(config_path)?;
    let check_cfg = cfg.check.as_ref();
    let ex = run_extraction(config_path)?;

    let mut symbols = crate::link::SymbolIndex::build(
        ex.trees
            .iter()
            .map(|t| (t.items.as_slice(), t.root_namespace.as_deref())),
    );
    for t in &ex.trees {
        symbols.add_presence(t.reference_names.iter().cloned());
    }
    let symbols = symbols;
    let mut findings = Vec::new();
    let mut stats = Vec::new();
    // A gate naming a tree that doesn't exist is silently off — the
    // config asserts an enforcement that isn't happening. Fail loudly.
    for (key, cfg) in [
        (
            "require_docs",
            check_cfg.and_then(|c| c.require_docs.as_ref()),
        ),
        (
            "require_param_docs",
            check_cfg.and_then(|c| c.require_param_docs.as_ref()),
        ),
    ] {
        if let Some(config::RequireDocs::Trees(v)) = cfg {
            for name in v {
                if !ex.trees.iter().any(|t| &t.prefix == name) {
                    bail!(
                        "[check] {key} names unknown tree `{name}`; configured trees: {}",
                        ex.trees
                            .iter()
                            .map(|t| t.prefix.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
    }
    let wants = |cfg: Option<&config::RequireDocs>, prefix: &String| match cfg {
        Some(config::RequireDocs::All(b)) => *b,
        Some(config::RequireDocs::Trees(v)) => v.contains(prefix),
        None => false,
    };
    for t in &ex.trees {
        let strict = crate::check::Strictness {
            require_docs: undocumented
                || wants(check_cfg.and_then(|c| c.require_docs.as_ref()), &t.prefix),
            require_param_docs: undocumented_params
                || wants(
                    check_cfg.and_then(|c| c.require_param_docs.as_ref()),
                    &t.prefix,
                ),
        };
        stats.push(crate::check::check_tree(
            &t.prefix,
            &t.items,
            &symbols,
            strict,
            &mut findings,
        ));
    }

    // Items that landed in no module are extracted (and counted above)
    // but the renderer has no page for them — silent invisibility the
    // config should resolve one way or the other.
    for t in &ex.trees {
        let mut orphan_files = std::collections::BTreeSet::new();
        fn collect_orphans(item: &Item, out: &mut std::collections::BTreeSet<String>) {
            if item.kind != ItemKind::Namespace
                && item.module.is_none()
                && let Some(src) = &item.source
            {
                out.insert(src.file.clone());
            }
            for c in &item.children {
                collect_orphans(c, out);
            }
        }
        for item in &t.items {
            collect_orphans(item, &mut orphan_files);
        }
        for f in orphan_files {
            findings.push(crate::check::Finding {
                lint: "unassigned",
                file: f,
                line: 0,
                message: format!(
                    "[{}] items from this header belong to no module and are not                      rendered; add it to the module map or drop it from the config",
                    t.prefix
                ),
            });
        }
    }

    // A binding map naming symbols that no longer exist is doc rot in
    // the cross-language links; its own header comment promises it
    // cannot rot silently.
    if let Some((map_path, warnings)) = &ex.binding_warnings {
        for w in warnings {
            findings.push(crate::check::Finding {
                lint: "binding-map",
                file: map_path.display().to_string(),
                line: 0,
                message: w.clone(),
            });
        }
    }

    if let Some(g) = &ex.guide {
        for (file, line, message) in crate::render::scan_rst_warnings(
            &g.dirs,
            &g.exclude,
            &symbols,
            g.include_root.as_deref(),
        )? {
            findings.push(crate::check::Finding {
                lint: "rst-dialect",
                file,
                line: line as u32,
                message,
            });
        }

        // Guide drift: hand-written signature directives that no longer
        // match the extracted API.
        let mut by_key: std::collections::HashMap<String, Vec<&Item>> =
            std::collections::HashMap::new();
        fn index_for_drift<'a>(
            item: &'a Item,
            map: &mut std::collections::HashMap<String, Vec<&'a Item>>,
        ) {
            if item.kind != ItemKind::Namespace {
                // Hand-written prose qualifies names at any depth
                // (`add_binary`, `AttributeContainer::add_binary`,
                // `PKCS11::AttributeContainer::add_binary`, ...): index
                // every scope suffix of the qualified name.
                let segments: Vec<&str> = item.qualified_name.split("::").collect();
                let mut keys: Vec<String> = (0..segments.len())
                    .map(|i| segments[i..].join("::"))
                    .collect();
                keys.push(item.name.clone());
                keys.sort();
                keys.dedup();
                for k in keys {
                    map.entry(k).or_default().push(item);
                }
            }
            for c in &item.children {
                index_for_drift(c, map);
            }
        }
        for t in &ex.trees {
            for item in &t.items {
                index_for_drift(item, &mut by_key);
            }
        }

        for (file, line, lang, sig) in crate::render::collect_sig_directives(&g.dirs, &g.exclude)? {
            let Some((name, count)) = crate::check::parse_handwritten_sig(&sig) else {
                continue;
            };
            // A Python item must not satisfy a cpp:function check and
            // vice versa — but C and C++ interchange freely: Botan's
            // ffi.rst documents its C API with cpp:function directives,
            // and ffi.h itself is a C++-parsed C surface.
            let compatible = |item_lang: Language| match lang {
                Language::Python => item_lang == Language::Python,
                _ => item_lang != Language::Python,
            };
            let candidates: Vec<&Item> = by_key
                .get(&name)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|i| compatible(i.lang))
                .copied()
                .collect();
            let candidates = candidates.as_slice();
            if candidates.is_empty() {
                findings.push(crate::check::Finding {
                    lint: "guide-drift",
                    file,
                    line: line as u32,
                    message: format!("documents `{name}`, which does not exist in the API"),
                });
                continue;
            }
            let (any, matched) = crate::check::sig_matches_any(count, candidates);
            if any && !matched {
                findings.push(crate::check::Finding {
                    lint: "guide-drift",
                    file,
                    line: line as u32,
                    message: format!(
                        "signature does not match any overload of `{name}` ({count} argument{})",
                        if count == 1 { "" } else { "s" }
                    ),
                });
            }
        }
    }

    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    for f in &findings {
        println!("{}:{}: [{}] {}", f.file, f.line, f.lint, f.message);
    }

    let mut failed = !findings.is_empty();
    let ratchet_path = check_cfg.and_then(|c| c.ratchet.as_ref()).map(|p| {
        if p.is_absolute() {
            p.clone()
        } else {
            base.join(p)
        }
    });
    // A configured ratchet is mandatory and fail-closed: a missing or
    // unparseable file must not silently disable enforcement. The one
    // exception is --update-ratchet, which exists to (re)create it.
    let recorded: std::collections::HashMap<String, f64> = match &ratchet_path {
        None => Default::default(),
        Some(p) => match std::fs::read_to_string(p) {
            Err(_) if update_ratchet => Default::default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
                "ratchet file {} not found; run `udg check --update-ratchet` to create it",
                p.display()
            ),
            Err(e) => return Err(e).context(format!("cannot read ratchet {}", p.display())),
            Ok(text) => match toml::from_str::<std::collections::HashMap<String, f64>>(&text) {
                Err(_) if update_ratchet => Default::default(),
                Err(e) => bail!("invalid ratchet file {}: {e}", p.display()),
                Ok(map) => {
                    for (tree, pct) in &map {
                        if !(0.0..=100.0).contains(pct) {
                            bail!(
                                "invalid ratchet file {}: `{tree}` = {pct} is not a percentage",
                                p.display()
                            );
                        }
                        if !stats.iter().any(|s| s.prefix == *tree) {
                            eprintln!(
                                "udg: warning: ratchet records `{tree}`, which is not a \
                                 configured tree — it is not being enforced"
                            );
                        }
                    }
                    map
                }
            },
        },
    };

    eprintln!("udg: coverage:");
    for st in &stats {
        let pct = st.percent();
        let mut notes = Vec::new();
        if let Some(min) = check_cfg.and_then(|c| c.min_coverage.get(&st.prefix))
            && pct + 1e-9 < *min
        {
            notes.push(format!("BELOW min_coverage {min:.1}%"));
            failed = true;
        }
        if let Some(prev) = recorded.get(&st.prefix)
            && pct + 0.05 < *prev
        {
            notes.push(format!("RATCHET REGRESSION (was {prev:.1}%)"));
            failed = true;
        }
        eprintln!(
            "udg:   {}: {}/{} ({pct:.1}%){}",
            st.prefix,
            st.documented,
            st.documentable,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {}", notes.join(", "))
            }
        );
    }
    eprintln!("udg: {} finding(s)", findings.len());

    if update_ratchet {
        if let Some(path) = &ratchet_path {
            let mut out = String::from(
                "# udg coverage ratchet: recorded coverage may never decrease.
",
            );
            for st in &stats {
                out.push_str(&format!(
                    "{} = {:.1}
",
                    st.prefix,
                    st.percent()
                ));
            }
            std::fs::write(path, out)
                .with_context(|| format!("cannot write {}", path.display()))?;
            eprintln!("udg: ratchet updated: {}", path.display());
        } else {
            bail!("--update-ratchet needs [check] ratchet in the config");
        }
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}

pub fn serve_cmd(config_path: &Path, port: u16) -> Result<()> {
    build_cmd(config_path, None, false)?;
    let (cfg, base) = config::load(config_path)?;
    let dir = cfg
        .output
        .as_ref()
        .map(|o| {
            if o.dir.is_absolute() {
                o.dir.clone()
            } else {
                base.join(&o.dir)
            }
        })
        .unwrap_or_else(|| PathBuf::from("build/docs"))
        .canonicalize()?;
    serve::serve(&dir, port)
}

/// One extracted language tree.
#[derive(Serialize)]
struct TreeData {
    /// Presence-only names for reference resolution (external headers,
    /// macros); never documented.
    reference_names: Vec<String>,
    title: String,
    prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_prefix: Option<String>,
    modules: Vec<Module>,
    items: Vec<Item>,
}

struct GuideCfg {
    title: String,
    dirs: Vec<PathBuf>,
    exclude: Vec<String>,
    default_code_language: Option<String>,
    include_root: Option<PathBuf>,
}

#[derive(Default)]
struct BrandingCfg {
    extra_css: Option<PathBuf>,
    logo: Option<PathBuf>,
    favicon: Option<PathBuf>,
    footer: Option<String>,
}

struct Extraction {
    project: String,
    trees: Vec<TreeData>,
    families: Vec<crate::link::Family>,
    /// (binding map path, stale-entry warnings): surfaced as check
    /// findings — a rotted binding map must not pass CI silently.
    binding_warnings: Option<(PathBuf, Vec<String>)>,
    guide: Option<GuideCfg>,
    branding: BrandingCfg,
    output_dir: Option<PathBuf>,
}

/// Top-level JSON document produced by `udg extract`.
#[derive(Serialize)]
struct ExtractReport {
    ir_version: u32,
    generator: String,
    project: String,
    trees: Vec<TreeData>,
}

/// Print extraction diagnostics; error diagnostics are fatal unless the
/// section opts out — they mean the AST (and every number derived from
/// it) is incomplete.
fn report_extraction_errors(
    section: &str,
    out: &crate::frontend::clang::ExtractOutput,
    allow: bool,
) -> Result<()> {
    for diag in &out.diagnostics {
        eprintln!("udg: {diag}");
    }
    for err in &out.errors {
        eprintln!("udg: {err}");
    }
    if !out.errors.is_empty() && !allow {
        bail!(
            "[{section}] extraction hit {} parse error(s); the AST is incomplete. \
             Fix include paths/flags (UDG_DEBUG_ARGS=1 prints the libclang argv) \
             or set [{section}] allow_parse_errors = true to proceed anyway",
            out.errors.len()
        );
    }
    Ok(())
}

/// Compile a section's `hide` globs, attributing errors to it.
fn hide_patterns(globs: &[String], section: &str) -> Result<Vec<glob::Pattern>> {
    globs
        .iter()
        .map(|g| {
            glob::Pattern::new(g)
                .map_err(|e| anyhow::anyhow!("[{section}] hide: invalid pattern `{g}`: {e}"))
        })
        .collect()
}

/// Drop items whose qualified name matches a `hide` pattern, along with
/// everything declared inside them — project conventions for non-public
/// API that lives in public headers (`detail` namespaces, `_`-prefixed
/// members).
fn apply_hide(items: &mut Vec<Item>, patterns: &[glob::Pattern]) {
    if patterns.is_empty() {
        return;
    }
    items.retain(|i| !patterns.iter().any(|p| p.matches(&i.qualified_name)));
    for i in items.iter_mut() {
        apply_hide(&mut i.children, patterns);
    }
}

fn run_extraction(config_path: &Path) -> Result<Extraction> {
    let (cfg, base) = config::load(config_path)?;
    let mut trees = Vec::new();

    // --- C++ tree ---------------------------------------------------------
    if let Some(cpp) = &cfg.cpp {
        let mut frontend_cfg = config::resolve_cpp(cpp, &base, None)?;
        let modules = match cfg.modules.as_ref().and_then(|m| m.cpp.as_ref()) {
            Some(map_ref) => modules::load(&map_ref.map, &base)?,
            None => Vec::new(),
        };
        if !modules.is_empty() {
            let mut headers = modules::all_headers(&modules);
            headers.extend(frontend_cfg.headers.iter().cloned());
            headers.dedup();
            frontend_cfg.headers = headers;
        }
        if frontend_cfg.headers.is_empty() {
            bail!("no C++ headers: set [cpp] headers or configure [modules.cpp]");
        }
        eprintln!(
            "udg: [cpp] parsing {} headers ({}, {} modules)",
            frontend_cfg.headers.len(),
            frontend_cfg.std,
            modules.len()
        );
        let out = crate::frontend::clang::extract(&frontend_cfg)?;
        report_extraction_errors("cpp", &out, cpp.allow_parse_errors)?;
        let mut items = out.items;
        apply_hide(&mut items, &hide_patterns(&cpp.hide, "cpp")?);
        crate::link::dedupe_alias_redeclarations(&mut items);
        // Projects without a module map get one flat module: the tool
        // must work before a project invests in structure metadata.
        let modules = if modules.is_empty() {
            vec![Module {
                id: "api".into(),
                name: "API Reference".into(),
                brief: None,
                parent: None,
                headers: frontend_cfg
                    .headers
                    .iter()
                    .map(|h| h.display().to_string())
                    .collect(),
            }]
        } else {
            modules
        };
        modules::assign(&mut items, &modules);
        print_stats("cpp", &items);
        trees.push(TreeData {
            reference_names: out.reference_names,
            title: "C++ API".into(),
            prefix: "cpp".into(),
            root_namespace: cfg.project.root_namespace.clone(),
            include_prefix: cpp.include_prefix.clone(),
            modules,
            items,
        });
    }

    // --- C tree -----------------------------------------------------------
    if let Some(c) = &cfg.c {
        let frontend_cfg = config::resolve_cpp(c, &base, Some(Language::C))?;
        if frontend_cfg.headers.is_empty() {
            bail!("[c] section has no headers");
        }
        eprintln!("udg: [c] parsing {} headers", frontend_cfg.headers.len());
        let out = crate::frontend::clang::extract(&frontend_cfg)?;
        report_extraction_errors("c", &out, c.allow_parse_errors)?;
        // Without a configured module map, each header is its own module.
        let modules: Vec<Module> = frontend_cfg
            .headers
            .iter()
            .map(|h| Module {
                id: h
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "c".into()),
                name: h
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "C API".into()),
                brief: None,
                parent: None,
                headers: vec![h.display().to_string()],
            })
            .collect();
        let mut items = out.items;
        apply_hide(&mut items, &hide_patterns(&c.hide, "c")?);
        crate::link::dedupe_alias_redeclarations(&mut items);
        modules::assign(&mut items, &modules);
        print_stats("c", &items);
        trees.push(TreeData {
            reference_names: out.reference_names,
            title: "C API".into(),
            prefix: "c".into(),
            root_namespace: cfg.project.root_namespace.clone(),
            include_prefix: c.include_prefix.clone(),
            modules,
            items,
        });
    }

    // --- Python tree ------------------------------------------------------
    if let Some(py) = &cfg.python {
        let py_cfg = crate::frontend::python::PyConfig {
            modules: py
                .modules
                .iter()
                .map(|p| {
                    if p.is_absolute() {
                        p.clone()
                    } else {
                        base.join(p)
                    }
                })
                .collect(),
        };
        eprintln!("udg: [python] parsing {} modules", py_cfg.modules.len());
        let out = crate::frontend::python::extract(&py_cfg)?;
        for w in &out.warnings {
            eprintln!("udg: {w}");
        }
        let mut items = out.items;
        apply_hide(&mut items, &hide_patterns(&py.hide, "python")?);
        print_stats("python", &items);
        trees.push(TreeData {
            reference_names: Vec::new(),
            title: "Python API".into(),
            prefix: "py".into(),
            root_namespace: None,
            include_prefix: None,
            modules: out.modules,
            items,
        });
    }

    if trees.is_empty() {
        bail!("nothing to document: configure [cpp], [c], and/or [python]");
    }

    // Linker: apply the cross-language binding map.
    let mut families = Vec::new();
    let mut binding_warnings = None;
    if let Some(b) = &cfg.bindings {
        let path = if b.map_file.is_absolute() {
            b.map_file.clone()
        } else {
            base.join(&b.map_file)
        };
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read binding map {}", path.display()))?;
        let roots: Vec<Option<String>> = trees.iter().map(|t| t.root_namespace.clone()).collect();
        let mut refs: Vec<(&mut Vec<Item>, Option<&str>)> = trees
            .iter_mut()
            .zip(&roots)
            .map(|(t, r)| (&mut t.items, r.as_deref()))
            .collect();
        let out = crate::link::apply_bindings(&text, &mut refs)?;
        for w in &out.warnings {
            eprintln!("udg: warning: {w}");
        }
        eprintln!(
            "udg: binding map applied ({} family edges)",
            out.families.len()
        );
        binding_warnings = Some((path, out.warnings));
        families = out.families;
    }

    let guide = cfg.guide.as_ref().map(|g| GuideCfg {
        title: g.title.clone(),
        dirs: g
            .src
            .iter()
            .map(|d| {
                if d.is_absolute() {
                    d.clone()
                } else {
                    base.join(d)
                }
            })
            .collect(),
        exclude: g.exclude.clone(),
        default_code_language: g.default_code_language.clone(),
        // Default literalinclude boundary: the config's directory (the
        // project checkout). Sphinx-era docs embed source examples from
        // across the tree (`/../src/examples/...`), and the project's
        // own root is the natural trust boundary.
        include_root: Some(
            g.include_root
                .as_ref()
                .map(|p| {
                    if p.is_absolute() {
                        p.clone()
                    } else {
                        base.join(p)
                    }
                })
                .unwrap_or_else(|| base.clone()),
        ),
    });

    let branding = cfg
        .output
        .as_ref()
        .map(|o| {
            let abs = |p: &PathBuf| {
                if p.is_absolute() {
                    p.clone()
                } else {
                    base.join(p)
                }
            };
            BrandingCfg {
                extra_css: o.extra_css.as_ref().map(&abs),
                logo: o.logo.as_ref().map(&abs),
                favicon: o.favicon.as_ref().map(&abs),
                footer: o.footer.clone(),
            }
        })
        .unwrap_or_default();

    Ok(Extraction {
        project: cfg.project.name.clone(),
        trees,
        families,
        binding_warnings,
        guide,
        branding,
        output_dir: cfg.output.as_ref().map(|o| {
            if o.dir.is_absolute() {
                o.dir.clone()
            } else {
                base.join(&o.dir)
            }
        }),
    })
}

pub fn extract_cmd(config_path: &Path, output: Option<&Path>) -> Result<()> {
    let ex = run_extraction(config_path)?;
    let report = ExtractReport {
        ir_version: 0,
        generator: format!("udg {}", env!("CARGO_PKG_VERSION")),
        project: ex.project,
        trees: ex.trees,
    };
    let json = serde_json::to_string_pretty(&report)?;
    match output {
        Some(path) => {
            std::fs::write(path, &json)
                .with_context(|| format!("cannot write {}", path.display()))?;
            eprintln!("udg: wrote {} ({} KiB)", path.display(), json.len() / 1024);
        }
        None => println!("{json}"),
    }
    Ok(())
}

/// Best-effort `--open`: hand the site to the platform opener; a
/// failure is a note, not an error — the build succeeded.
fn open_in_browser(path: &Path) {
    #[cfg(target_os = "macos")]
    const OPENER: &str = "open";
    #[cfg(target_os = "windows")]
    const OPENER: &str = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    const OPENER: &str = "xdg-open";
    match std::process::Command::new(OPENER).arg(path).spawn() {
        Ok(_) => eprintln!("udg: opening {}", path.display()),
        Err(e) => eprintln!("udg: cannot open {} with {OPENER}: {e}", path.display()),
    }
}

pub fn build_cmd(config_path: &Path, output: Option<&Path>, open: bool) -> Result<()> {
    let ex = run_extraction(config_path)?;
    let out_dir = output
        .map(Path::to_path_buf)
        .or(ex.output_dir)
        .unwrap_or_else(|| PathBuf::from("build/docs"));

    let reference_names: Vec<String> = ex
        .trees
        .iter()
        .flat_map(|t| t.reference_names.iter().cloned())
        .collect();
    let input = crate::render::RenderInput {
        project: &ex.project,
        reference_names: &reference_names,
        trees: ex
            .trees
            .iter()
            .map(|t| crate::render::TreeInput {
                title: &t.title,
                prefix: &t.prefix,
                root_namespace: t.root_namespace.as_deref(),
                include_prefix: t.include_prefix.as_deref(),
                modules: &t.modules,
                items: &t.items,
            })
            .collect(),
        families: &ex.families,
        guide: ex.guide.as_ref().map(|g| crate::render::GuideInput {
            title: &g.title,
            dirs: &g.dirs,
            exclude: &g.exclude,
            default_code_language: g.default_code_language.as_deref(),
            include_root: g.include_root.as_deref(),
        }),
        branding: crate::render::Branding {
            extra_css: ex.branding.extra_css.as_deref(),
            logo: ex.branding.logo.as_deref(),
            favicon: ex.branding.favicon.as_deref(),
            footer: ex.branding.footer.as_deref(),
        },
    };
    let result = crate::render::render(&input, &out_dir)?;
    for w in &result.warnings {
        eprintln!("udg: warning: {w}");
    }
    eprintln!(
        "udg: rendered {} pages ({} search entries) to {}",
        result.pages_written,
        result.search_entries,
        out_dir.display()
    );

    if open {
        let index = out_dir.join("index.html");
        open_in_browser(&index);
    }
    Ok(())
}

fn print_stats(label: &str, items: &[Item]) {
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut documentable = 0usize;
    let mut documented = 0usize;
    for item in items {
        item.walk(&mut |i| {
            *by_kind.entry(format!("{:?}", i.kind)).or_default() += 1;
            if !matches!(i.kind, ItemKind::Namespace | ItemKind::Enumerator) {
                documentable += 1;
                if i.docs.as_ref().is_some_and(|d| d.summary.is_some()) {
                    documented += 1;
                }
            }
        });
    }
    let counts: Vec<String> = by_kind.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    eprintln!("udg: [{label}] extracted {}", counts.join(", "));
    if documentable > 0 {
        eprintln!(
            "udg: [{label}] doc coverage {}/{} ({:.0}%)",
            documented,
            documentable,
            100.0 * documented as f64 / documentable as f64
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Language;

    #[test]
    fn hide_prunes_matching_subtrees_and_members() {
        let mk = |kind, name: &str, q: &str| Item::new(Language::Cpp, kind, name, q);
        let mut detail = mk(ItemKind::Namespace, "detail", "Botan::detail");
        detail
            .children
            .push(mk(ItemKind::Class, "Impl", "Botan::detail::Impl"));
        let mut group = mk(ItemKind::Class, "EC_Group", "Botan::EC_Group");
        group
            .children
            .push(mk(ItemKind::Method, "_data", "Botan::EC_Group::_data"));
        group
            .children
            .push(mk(ItemKind::Method, "get_p", "Botan::EC_Group::get_p"));
        let mut root = mk(ItemKind::Namespace, "Botan", "Botan");
        root.children.push(detail);
        root.children.push(group);
        let mut items = vec![root];

        let patterns = hide_patterns(&["*::detail".into(), "*::_*".into()], "cpp").unwrap();
        apply_hide(&mut items, &patterns);

        let botan = &items[0];
        assert_eq!(botan.children.len(), 1, "detail namespace pruned");
        let group = &botan.children[0];
        assert_eq!(group.name, "EC_Group");
        let names: Vec<&str> = group.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["get_p"], "underscore member pruned");

        assert!(hide_patterns(&["[".into()], "cpp").is_err());
    }
}
