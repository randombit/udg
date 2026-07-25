//! Guide (prose) pages: a directory of Markdown files rendered with
//! API autolinks, listed in their own nav tree.

use std::collections::HashMap;
use std::path::Path;

use crate::link::{Resolution, SymbolIndex};
use anyhow::{Context, Result};
use minijinja::{Environment, context};

use crate::ir::{Item, ItemKind};

use crate::render::md::{Md, Resolver};
use crate::render::pages::SearchEntry;
use crate::render::views::{Crumb, docs_view};
use crate::render::{html_escape, root_prefix, sig};

pub struct GuideInput<'a> {
    pub title: &'a str,
    /// Sphinx highlight_language equivalent: language for bare `::`
    /// literal blocks in rst pages.
    pub default_code_language: Option<&'a str>,
    /// Source directories of .md and .rst pages, merged.
    pub dirs: &'a [std::path::PathBuf],
    /// File stems to skip (Sphinx scaffolding like `contents`, `index`).
    pub exclude: &'a [String],
    /// Filesystem boundary for `literalinclude`; the CLI defaults it
    /// to the config's directory (the project checkout root).
    pub include_root: Option<&'a std::path::Path>,
}

struct GuidePage {
    stem: String,
    title: String,
    /// Pre-rendered body HTML (guide pages all live at depth 1, so one
    /// resolver with root "../" serves every page).
    html: String,
}

/// One source file, page or not: excluded files (Sphinx scaffolding
/// like contents.rst) still contribute their toctree to nav ordering.
struct SourceFile {
    path: std::path::PathBuf,
    stem: String,
    excluded: bool,
    title: String,
    html: String,
    toctree: Vec<(usize, String)>,
}

/// `:ref:` label -> (page url, section title).
type RefTargets = HashMap<String, (String, Option<String>)>;

/// Cross-file link registries scanned from the raw sources before
/// rendering: explicit `.. _label:` targets plus autosectionlabel
/// section titles, and document stems -> page titles. Both resolve
/// `:ref:` / `:doc:` roles.
fn link_registries(
    dirs: &[std::path::PathBuf],
    exclude: &[String],
) -> Result<(RefTargets, HashMap<String, String>)> {
    let heading_after = |lines: &[&str]| -> Option<String> {
        lines.windows(2).find_map(|w| {
            let text = w[0].trim_end();
            if !text.is_empty()
                && !text.starts_with("..")
                && crate::render::rst::adornment_char(w[1]).is_some()
                && w[1].trim_end().len() >= text.trim_start().len()
            {
                Some(text.trim_start().to_owned())
            } else {
                None
            }
        })
    };
    let mut targets = HashMap::new();
    let mut docs = HashMap::new();
    for path in collect_paths(dirs)? {
        if path.extension().is_none_or(|e| e != "rst") {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Excluded pages are not rendered; offering them as link
        // targets would produce dead hrefs.
        if exclude.contains(&stem) {
            continue;
        }
        let raw = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = raw.lines().collect();
        docs.insert(
            stem.clone(),
            heading_after(&lines).unwrap_or_else(|| stem.clone()),
        );
        for (i, l) in lines.iter().enumerate() {
            if let Some(label) = l
                .trim()
                .strip_prefix(".. _")
                .and_then(|r| r.strip_suffix(':'))
            {
                targets.insert(
                    label.trim().to_owned(),
                    (format!("{stem}.html"), heading_after(&lines[i + 1..])),
                );
            }
        }
        // Sphinx autosectionlabel (prefix_document form): every section
        // title is a target named `docname:lowercased title`.
        for w in lines.windows(2) {
            let text = w[0].trim();
            if !text.is_empty()
                && !text.starts_with("..")
                && crate::render::rst::adornment_char(w[1]).is_some()
                && w[1].trim_end().len() >= text.len()
            {
                targets
                    .entry(format!("{stem}:{}", text.to_lowercase()))
                    .or_insert_with(|| (format!("{stem}.html"), Some(text.to_owned())));
            }
        }
    }
    Ok((targets, docs))
}

/// All .md/.rst files across the guide dirs, sorted.
fn collect_paths(dirs: &[std::path::PathBuf]) -> Result<Vec<std::path::PathBuf>> {
    let mut entries = Vec::new();
    for dir in dirs {
        let listing = std::fs::read_dir(dir)
            .with_context(|| format!("cannot read guide dir {}", dir.display()))?;
        entries.extend(
            listing
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "md" || e == "rst")),
        );
    }
    entries.sort();
    Ok(entries)
}

/// rst dialect warnings for every non-excluded guide page, as
/// (file, line, message) — consumed by `udg check`.
/// Resolve a transclusion request to an item id: the language the
/// directive pins (autodoc = Python), or the cross-language preference
/// order for `udg:member`. Shared by the build-time embed and the
/// `udg check` rst lint so both agree on what resolves.
fn resolve_embed(symbols: &SymbolIndex, req: &crate::render::rst::EmbedRequest) -> Option<String> {
    let prefs: Vec<Option<crate::ir::Language>> = match req.lang {
        Some(l) => vec![Some(l)],
        None => vec![
            None,
            Some(crate::ir::Language::Cpp),
            Some(crate::ir::Language::C),
            Some(crate::ir::Language::Python),
        ],
    };
    prefs
        .into_iter()
        .find_map(|lang| match symbols.resolve(req.name, None, lang) {
            Resolution::Unique(id) => Some(id),
            _ => None,
        })
}

pub fn scan_rst_warnings(
    dirs: &[std::path::PathBuf],
    exclude: &[String],
    symbols: &SymbolIndex,
    include_root: Option<&std::path::Path>,
) -> Result<Vec<(String, usize, String)>> {
    // The lint must judge everything the way the build does: resolve
    // transclusions and :ref:/:doc: targets for real, render nothing.
    let embed = |req: &crate::render::rst::EmbedRequest| -> Option<String> {
        resolve_embed(symbols, req).map(|_| String::new())
    };
    // Presence resolver for the lint: ambiguous or partially
    // qualified names exist (they merely fail to produce a link), so
    // only names unknown at every depth count as rot.
    let resolve = |name: &str| -> Option<String> { symbols.exists(name).then(String::new) };
    let res: Option<crate::render::md::Resolver> = Some(&resolve);
    let (ref_targets, doc_pages) = link_registries(dirs, exclude)?;
    let known: std::collections::HashSet<String> = collect_paths(dirs)?
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    let mut out = Vec::new();
    for path in collect_paths(dirs)? {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if exclude.contains(&stem) || path.extension().is_none_or(|e| e != "rst") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)?;
        let opts = crate::render::rst::RstOptions {
            file: Some(&path),
            source_root: dirs.first().map(|d| d.as_path()),
            embed: Some(&embed),
            resolve: res,
            ref_targets: Some(&ref_targets),
            doc_pages: Some(&doc_pages),
            include_root,
            ..crate::render::rst::RstOptions::default()
        };
        let rendered = crate::render::rst::render(&raw, &opts);
        for w in rendered.warnings {
            out.push((path.display().to_string(), w.line, w.message));
        }
        for (line, entry) in rendered.toctree {
            let entry_stem = entry.rsplit('/').next().unwrap_or(&entry);
            if !known.contains(entry_stem) {
                out.push((
                    path.display().to_string(),
                    line,
                    format!("toctree entry `{entry}` matches no guide page"),
                ));
            }
        }
    }
    Ok(out)
}

/// Hand-written signature directives from every non-excluded rst page,
/// as (file, line, signature) — input to the guide-drift lint.
pub fn collect_sig_directives(
    dirs: &[std::path::PathBuf],
    exclude: &[String],
) -> Result<Vec<(String, usize, crate::ir::Language, String)>> {
    let mut out = Vec::new();
    for path in collect_paths(dirs)? {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if exclude.contains(&stem) || path.extension().is_none_or(|e| e != "rst") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)?;
        let rendered = crate::render::rst::render(&raw, &crate::render::rst::RstOptions::default());
        for (line, lang, sig) in rendered.sig_directives {
            out.push((path.display().to_string(), line, lang, sig));
        }
    }
    Ok(out)
}

/// Depth-first walk of toctree references, assigning nav positions.
/// Entries resolve relative to the referencing file; excluded files
/// (contents pages) contribute ordering without becoming pages.
fn toctree_order(files: &[SourceFile], dirs: &[std::path::PathBuf]) -> HashMap<String, usize> {
    let by_path: HashMap<std::path::PathBuf, usize> = files
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.path.canonicalize().ok().map(|p| (p, i)))
        .collect();

    fn visit(
        idx: usize,
        files: &[SourceFile],
        by_path: &HashMap<std::path::PathBuf, usize>,
        order: &mut HashMap<String, usize>,
        visited: &mut std::collections::HashSet<usize>,
    ) {
        if !visited.insert(idx) {
            return;
        }
        let parent = files[idx].path.parent().map(std::path::Path::to_path_buf);
        for (_, entry) in &files[idx].toctree {
            let Some(parent) = &parent else { continue };
            let child = ["rst", "md"].iter().find_map(|ext| {
                parent
                    .join(format!("{entry}.{ext}"))
                    .canonicalize()
                    .ok()
                    .and_then(|p| by_path.get(&p).copied())
            });
            let Some(child) = child else { continue };
            if !files[child].excluded {
                let next = order.len();
                order.entry(files[child].stem.clone()).or_insert(next);
            }
            visit(child, files, by_path, order, visited);
        }
    }

    // The master document is the contents/index file of the FIRST
    // configured dir; other contents files rank by their dir's position.
    let canon_dirs: Vec<Option<std::path::PathBuf>> =
        dirs.iter().map(|d| d.canonicalize().ok()).collect();
    let dir_rank = |f: &SourceFile| -> usize {
        let parent = f.path.parent().and_then(|p| p.canonicalize().ok());
        canon_dirs
            .iter()
            .position(|d| d.is_some() && *d == parent)
            .unwrap_or(usize::MAX)
    };
    let mut roots: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.stem.as_str(), "contents" | "index") && !f.toctree.is_empty())
        .map(|(i, _)| i)
        .collect();
    roots.sort_by_key(|&i| (dir_rank(&files[i]), files[i].stem != "contents"));

    let mut order = HashMap::new();
    let mut visited = std::collections::HashSet::new();
    for i in roots {
        visit(i, files, &by_path, &mut order, &mut visited);
    }
    order
}

pub struct GuideOutput {
    pub pages_written: usize,
    /// Landing-page list HTML.
    pub list_html: String,
    pub warnings: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn write_guides(
    env: &Environment,
    out_dir: &Path,
    guide: &GuideInput,
    project: &str,
    others: &[(String, String)],
    symbols: &SymbolIndex,
    id_url: &HashMap<String, String>,
    items_by_id: &HashMap<String, &Item>,
    search: &mut Vec<SearchEntry>,
) -> Result<GuideOutput> {
    let md = Md::new();
    let root = "../";
    let resolve = |name: &str| -> Option<String> {
        match symbols.resolve(name, None, None) {
            Resolution::Unique(id) => id_url.get(&id).map(|u| format!("{root}{u}")),
            _ => None,
        }
    };
    let res: Option<Resolver> = Some(&resolve);

    // Transclusion (`.. udg:member::` and Sphinx autodoc): the machine-
    // generated member block, so guides never hand-copy tactical docs.
    // udg:member resolves ambiguous names preferring C++, then C, then
    // Python; autodoc pins Python (it is the Sphinx Python domain).
    let embed_one = |item: &Item| -> Option<String> {
        let chunks = match item.kind {
            ItemKind::Class | ItemKind::Struct => sig::for_record_chunks(item),
            ItemKind::TypeAlias | ItemKind::Variable | ItemKind::Field => {
                sig::for_leaf_chunks(item)
            }
            _ => sig::for_function_chunks(item),
        };
        let sig_html = crate::render::linkify_chunks(&chunks, res);
        let docs = item
            .docs
            .as_ref()
            .map(|d| docs_view(d, item.deprecated.as_deref(), &md, res));
        let ref_url = id_url.get(&item.id).map(|u| format!("{root}{u}"));
        env.get_template("embed.html")
            .ok()?
            .render(minijinja::context! {
                root => root,
                sig_html => sig_html,
                since => item.since,
                deprecated => item.deprecated.is_some(),
                docs => docs,
                ref_url => ref_url,
            })
            .ok()
    };
    let embed = |req: &crate::render::rst::EmbedRequest| -> Option<String> {
        let id = resolve_embed(symbols, req)?;
        let item = items_by_id.get(&id)?;
        let mut html = embed_one(item)?;
        if let Some(want) = &req.members {
            // Sphinx `:members:` semantics: bare -> every public,
            // documented member; with names -> exactly those.
            let picked = item.children.iter().filter(|c| {
                if c.access == Some(crate::ir::Access::Private) {
                    return false;
                }
                if want.is_empty() {
                    c.docs.is_some() && !c.name.starts_with('_')
                } else {
                    want.iter().any(|w| w == &c.name)
                }
            });
            let member_html: String = picked.filter_map(embed_one).collect();
            if !member_html.is_empty() {
                html.push_str("<div class=\"embed-members\">\n");
                html.push_str(&member_html);
                html.push_str("</div>\n");
            }
        }
        Some(html)
    };

    let (ref_targets, doc_pages) = link_registries(guide.dirs, guide.exclude)?;
    let mut warnings: Vec<String> = Vec::new();
    let mut files: Vec<SourceFile> = Vec::new();
    for path in collect_paths(guide.dirs)? {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let excluded = guide.exclude.contains(&stem);
        let raw = std::fs::read_to_string(&path)?;
        let (title, html, toctree) = if path.extension().is_some_and(|e| e == "rst") {
            let out = crate::render::rst::render(
                &raw,
                &crate::render::rst::RstOptions {
                    resolve: res,
                    embed: Some(&embed),
                    default_lang: guide.default_code_language,
                    file: Some(&path),
                    source_root: guide.dirs.first().map(|d| d.as_path()),
                    ref_targets: Some(&ref_targets),
                    doc_pages: Some(&doc_pages),
                    include_root: guide.include_root,
                },
            );
            if !excluded {
                warnings.extend(
                    out.warnings
                        .iter()
                        .map(|w| format!("{}:{}: {}", path.display(), w.line, w.message)),
                );
            }
            (
                out.title.unwrap_or_else(|| stem.clone()),
                out.html,
                out.toctree,
            )
        } else {
            let title = raw
                .lines()
                .find_map(|l| l.strip_prefix("# "))
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(&stem)
                .to_owned();
            (title, md.html_with(&raw, res), Vec::new())
        };
        files.push(SourceFile {
            path,
            stem,
            excluded,
            title,
            html,
            toctree,
        });
    }

    // Imported docs must not disappear silently: a toctree entry that
    // matches no discovered file is a dead nav reference.
    let known: std::collections::HashSet<&str> = files.iter().map(|f| f.stem.as_str()).collect();
    for f in &files {
        for (line, entry) in &f.toctree {
            let stem = entry.rsplit('/').next().unwrap_or(entry);
            if !known.contains(stem) {
                warnings.push(format!(
                    "{}:{line}: toctree entry `{entry}` matches no guide page",
                    f.path.display()
                ));
            }
        }
    }

    // Nav order: toctree references first, alphabetical for the rest.
    let order = toctree_order(&files, guide.dirs);
    let mut pages: Vec<GuidePage> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for f in &files {
        if f.excluded {
            continue;
        }
        if !seen.insert(f.stem.clone()) {
            warnings.push(format!(
                "guide: duplicate page stem `{}`, skipping {}",
                f.stem,
                f.path.display()
            ));
            continue;
        }
        pages.push(GuidePage {
            stem: f.stem.clone(),
            title: f.title.clone(),
            html: f.html.clone(),
        });
    }
    pages.sort_by_key(|p| {
        (
            order.get(&p.stem).copied().unwrap_or(usize::MAX),
            p.title.to_lowercase(),
        )
    });

    // Sidebar nav shared by all guide pages, written once as nav.js
    // (script-tag injected; the loader marks the current page).
    let nav_data = {
        let mut out = format!(
            "<div class=\"navtitle\"><a href=\"guide/index.html\">{}</a></div><ul class=\"navtree\">",
            html_escape(guide.title)
        );
        for p in &pages {
            out.push_str(&format!(
                "<li><a href=\"guide/{}.html\">{}</a></li>",
                p.stem,
                html_escape(&p.title)
            ));
        }
        out.push_str("</ul><div class=\"navtitle other\">Other APIs</div><ul class=\"navtree\">");
        for (title, prefix) in others {
            if prefix != "guide" {
                out.push_str(&format!(
                    "<li><a href=\"{prefix}/index.html\">{}</a></li>",
                    html_escape(title)
                ));
            }
        }
        out.push_str("</ul>");
        out
    };
    std::fs::create_dir_all(out_dir.join("guide"))?;
    std::fs::write(
        out_dir.join("guide/nav.js"),
        format!("window.UDG_NAV = {};\n", serde_json::to_string(&nav_data)?),
    )?;

    let mut written = 0usize;
    for page in &pages {
        let url = format!("guide/{}.html", page.stem);
        let root = root_prefix(&url);
        let body = page.html.clone();
        let html = env.get_template("guide.html")?.render(context! {
            project => project,
            title => page.title,
            root => root,
            nav_src => format!("{root}guide/nav.js"),
            nav_current => url.clone(),
            crumbs => vec![
                Crumb { name: guide.title.into(), url: "guide/index.html".into() },
                Crumb { name: page.title.clone(), url: String::new() },
            ],
            body_html => body,
        })?;
        crate::render::pages::write_file(out_dir, &url, &html)?;
        written += 1;

        search.push(SearchEntry {
            n: page.title.clone(),
            q: page.title.clone(),
            k: "guide".into(),
            u: url,
            m: "guide".into(),
        });
    }

    // Guide index: the page list.
    let list_html = {
        let mut out = String::from("<ul class=\"modtree\">");
        for p in &pages {
            out.push_str(&format!(
                "<li><a href=\"guide/{}.html\">{}</a></li>",
                p.stem,
                html_escape(&p.title)
            ));
        }
        out.push_str("</ul>");
        out
    };
    let index_root = "../";
    let mut index_list = String::from("<ul class=\"modtree\">");
    for p in &pages {
        index_list.push_str(&format!(
            "<li><a href=\"{index_root}guide/{}.html\">{}</a></li>",
            p.stem,
            html_escape(&p.title)
        ));
    }
    index_list.push_str("</ul>");
    let html = env.get_template("index.html")?.render(context! {
        project => project,
        title => format!("{} {}", project, guide.title),
        root => index_root,
        nav_src => format!("{index_root}guide/nav.js"),
        nav_current => "guide/index.html",
        crumbs => Vec::<Crumb>::new(),
        stats => format!("{} pages", pages.len()),
        tree_html => index_list,
    })?;
    crate::render::pages::write_file(out_dir, "guide/index.html", &html)?;
    written += 1;

    Ok(GuideOutput {
        pages_written: written,
        list_html,
        warnings,
    })
}
