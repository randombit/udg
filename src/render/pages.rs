//! Page planning and writing: which pages exist, at which URLs, and the
//! view models each one gets.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::ir::{Access, Item, ItemKind, Module};
use anyhow::{Context, Result};
use minijinja::{Environment, context};
use serde::Serialize;

use crate::link::{Resolution, SymbolIndex};

use crate::render::md::{Md, Resolver};
use crate::render::views::{BindingLink, Crumb, DocsView, MemberView, Row, Section, docs_view};
use crate::render::{TreeInput, html_escape, root_prefix, sanitize, sig};

/// Link-resolution context shared by every page of every tree.
pub struct LinkCtx<'x> {
    pub symbols: &'x SymbolIndex,
    /// item id -> site-root-relative URL (with #anchor for members).
    pub id_url: &'x HashMap<String, String>,
    pub families: &'x [crate::link::Family],
    /// base item id -> classes deriving from it (reverse inheritance).
    pub derived: &'x HashMap<String, Vec<DerivedEntry>>,
    /// Every item in every tree, by id (for base-class member lookup).
    pub items_by_id: &'x HashMap<String, &'x Item>,
}

/// One known subclass of a base, collected across all trees.
pub struct DerivedEntry {
    pub display: String,
    pub id: String,
    pub summary: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum PageKind {
    Record,
    Enum,
    Leaf,
}

struct Page<'a> {
    url: String,
    kind: PageKind,
    title: String,
    module: String,
    items: Vec<&'a Item>,
}

#[derive(Serialize)]
pub struct SearchEntry {
    pub(crate) n: String,
    pub(crate) q: String,
    pub(crate) k: String,
    pub(crate) u: String,
    pub(crate) m: String,
}

#[derive(Serialize)]
struct MemberSection {
    title: String,
    members: Vec<MemberView>,
}

#[derive(Serialize)]
struct VariantView {
    anchor: String,
    name: String,
    value: Option<String>,
    deprecated: bool,
    dep_html: Option<String>,
    summary_html: Option<String>,
}

pub struct Site<'a> {
    project: &'a str,
    title: &'a str,
    prefix: &'a str,
    root_ns: Option<&'a str>,
    include_prefix: Option<&'a str>,
    /// (title, prefix) of every tree in the site, for cross-tree nav.
    others: &'a [(String, String)],
    modules: &'a [Module],
    md: Md,
    pages: BTreeMap<String, Page<'a>>,
    /// Free operators attached to class pages: record item id -> ops,
    /// sorted and deduplicated. An operator taking two documented types
    /// appears on both pages.
    related_ops: HashMap<String, Vec<&'a Item>>,
    /// Pointer-identity set of operators claimed by a class page (kept
    /// off the module's function leaf pages).
    attached_ops: std::collections::HashSet<usize>,
    module_names: HashMap<String, String>,
    source_urls: HashMap<PathBuf, String>,
    warnings: Vec<String>,
}

fn kind_prefix(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Class => "class",
        ItemKind::Struct => "struct",
        ItemKind::Enum => "enum",
        ItemKind::Function => "fn",
        ItemKind::TypeAlias => "type",
        ItemKind::Macro => "macro",
        _ => "const",
    }
}

fn kind_chip(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Class => "class",
        ItemKind::Struct => "struct",
        ItemKind::Enum => "enum",
        // Reachable only for enumerators hoisted out of unnamed enums,
        // which read as constants of the enclosing scope.
        ItemKind::Enumerator => "const",
        ItemKind::Function => "fn",
        ItemKind::Method | ItemKind::ConversionFunction => "method",
        ItemKind::Constructor => "ctor",
        ItemKind::Destructor => "dtor",
        ItemKind::TypeAlias => "type",
        ItemKind::Variable | ItemKind::Field => "const",
        ItemKind::Macro => "macro",
        _ => "item",
    }
}

/// Display name: qualified name without the project's root namespace
/// (`root_ns`, from config — e.g. `Botan`).
fn short_name<'q>(qualified: &'q str, root_ns: Option<&str>) -> &'q str {
    match root_ns {
        Some(ns) => qualified
            .strip_prefix(ns)
            .and_then(|r| r.strip_prefix("::"))
            .unwrap_or(qualified),
        None => qualified,
    }
}

fn page_url(
    prefix: &str,
    kind: ItemKind,
    module: &str,
    qualified: &str,
    root_ns: Option<&str>,
) -> String {
    let name = short_name(qualified, root_ns).replace("::", ".");
    format!(
        "{prefix}/{module}/{}.{}.html",
        kind_prefix(kind),
        sanitize(&name)
    )
}

/// One URL per source file, computed up front in module order so
/// same-basename headers inside one module get deterministic `.2`,
/// `.3` suffixes instead of two rayon workers racing one path.
fn plan_source_urls(prefix: &str, modules: &[Module]) -> HashMap<PathBuf, String> {
    let mut urls: HashMap<PathBuf, String> = HashMap::new();
    for m in modules {
        let mut seen: HashMap<String, u32> = HashMap::new();
        for h in &m.headers {
            let Ok(canon) = PathBuf::from(h).canonicalize() else {
                continue;
            };
            let base = canon
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unknown".into());
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            let url = if *n == 1 {
                format!("{prefix}/{}/src.{}.html", m.id, sanitize(&base))
            } else {
                format!("{prefix}/{}/src.{}.{n}.html", m.id, sanitize(&base))
            };
            // A header shared by two modules keeps its first URL.
            urls.entry(canon).or_insert(url);
        }
    }
    urls
}

impl<'a> Site<'a> {
    pub fn build(
        project: &'a str,
        tree: &TreeInput<'a>,
        others: &'a [(String, String)],
    ) -> Site<'a> {
        let mut site = Site {
            project,
            title: tree.title,
            prefix: tree.prefix,
            root_ns: tree.root_namespace,
            include_prefix: tree.include_prefix,
            others,
            modules: tree.modules,
            md: Md::new(),
            pages: BTreeMap::new(),
            related_ops: HashMap::new(),
            attached_ops: std::collections::HashSet::new(),
            module_names: tree
                .modules
                .iter()
                .map(|m| (m.id.clone(), m.name.clone()))
                .collect(),
            source_urls: plan_source_urls(tree.prefix, tree.modules),
            warnings: Vec::new(),
        };
        site.compute_related_ops(tree.items);
        site.collect_scope(tree.items);
        site
    }

    /// Find free `operator...` functions and attach them to the record
    /// pages of the documented types in their parameter lists.
    fn compute_related_ops(&mut self, items: &'a [Item]) {
        if !matches!(
            items.first().map(|i| i.lang),
            Some(crate::ir::Language::Cpp)
        ) {
            return;
        }
        // Record name index: short qualified name and last name segment.
        let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
        for top in items {
            walk_items(top, &mut |i| {
                if matches!(i.kind, ItemKind::Class | ItemKind::Struct | ItemKind::Enum) {
                    let short = short_name(&i.qualified_name, self.root_ns).to_owned();
                    by_key.entry(short.clone()).or_default().push(i.id.clone());
                    if let Some(last) = short.rsplit("::").next()
                        && last != short
                    {
                        by_key
                            .entry(last.to_owned())
                            .or_default()
                            .push(i.id.clone());
                    }
                }
            });
        }
        let unique = |key: &str| -> Option<&String> {
            match by_key.get(key).map(Vec::as_slice) {
                Some([id]) => Some(id),
                _ => None,
            }
        };

        for top in items {
            walk_items(top, &mut |op| {
                if op.kind != ItemKind::Function || !op.name.starts_with("operator") {
                    return;
                }
                let Some(sig) = &op.signature else { return };
                let mut targets: Vec<String> = Vec::new();
                for param in &sig.params {
                    for tok in ident_tokens(&param.ty) {
                        let hit = unique(&tok).or_else(|| tok.rsplit("::").next().and_then(unique));
                        if let Some(id) = hit
                            && !targets.contains(id)
                        {
                            targets.push(id.clone());
                        }
                    }
                }
                if targets.is_empty() {
                    return;
                }
                self.attached_ops.insert(op as *const Item as usize);
                for id in targets {
                    self.related_ops.entry(id).or_default().push(op);
                }
            });
        }
        for ops in self.related_ops.values_mut() {
            ops.sort_by_key(|o| (o.name.clone(), sig::for_function(o)));
            ops.dedup_by_key(|o| sig::for_function(o));
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn stats_line(&self) -> String {
        let documented: usize = self
            .pages
            .values()
            .flat_map(|p| &p.items)
            .filter(|i| i.docs.is_some())
            .count();
        format!(
            "{} modules · {} pages · {} documented top-level items",
            self.modules.len(),
            self.pages.len(),
            documented
        )
    }

    /// Precompute item-id -> URL (with member anchors) for the whole
    /// tree, for autolink and binding resolution. Anchor assignment here
    /// MUST match the per-page logic in record_view/enum_view: one pass
    /// over children in declaration order, one counter per anchor base.
    pub fn plan_urls(&self) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = HashMap::new();
        for page in self.pages.values() {
            let item = page.items[0];
            map.entry(item.id.clone())
                .or_insert_with(|| page.url.clone());
            match page.kind {
                PageKind::Record => {
                    let mut anchors: HashMap<String, u32> = HashMap::new();
                    for child in &item.children {
                        if child.access == Some(Access::Private) {
                            continue;
                        }
                        let base = match child.kind {
                            ItemKind::Constructor
                            | ItemKind::Method
                            | ItemKind::ConversionFunction => {
                                format!("m.{}", sanitize(&child.name))
                            }
                            ItemKind::Destructor
                                if child.docs.is_some() || child.deprecated.is_some() =>
                            {
                                format!("m.{}", sanitize(&child.name))
                            }
                            ItemKind::Field | ItemKind::Variable => {
                                format!("f.{}", sanitize(&child.name))
                            }
                            ItemKind::TypeAlias => format!("t.{}", sanitize(&child.name)),
                            ItemKind::Enumerator => format!("e.{}", sanitize(&child.name)),
                            _ => continue,
                        };
                        let n = anchors.entry(base.clone()).or_insert(0);
                        *n += 1;
                        let anchor = if *n == 1 { base } else { format!("{base}.{n}") };
                        map.entry(child.id.clone())
                            .or_insert_with(|| format!("{}#{anchor}", page.url));
                    }
                }
                PageKind::Enum => {
                    for child in &item.children {
                        map.entry(child.id.clone())
                            .or_insert_with(|| format!("{}#e.{}", page.url, sanitize(&child.name)));
                    }
                }
                PageKind::Leaf => {}
            }
            // Non-member operators attached to this record page (must
            // mirror record_view's order and dedup exactly).
            if matches!(page.kind, PageKind::Record | PageKind::Enum)
                && let Some(ops) = self.related_ops.get(&item.id)
            {
                let mut anchors: HashMap<String, u32> = HashMap::new();
                let mut seen_sigs = std::collections::HashSet::new();
                for op in ops {
                    if !seen_sigs.insert(sig::for_function(op)) {
                        continue;
                    }
                    let base = format!("nm.{}", sanitize(&op.name));
                    let n = anchors.entry(base.clone()).or_insert(0);
                    *n += 1;
                    let anchor = if *n == 1 { base } else { format!("{base}.{n}") };
                    map.entry(op.id.clone())
                        .or_insert_with(|| format!("{}#{anchor}", page.url));
                }
            }
        }
        map
    }

    fn collect_scope(&mut self, items: &'a [Item]) {
        for item in items {
            if item.kind == ItemKind::Namespace {
                self.collect_scope(&item.children);
            } else {
                self.add_page_item(item);
            }
        }
    }

    fn add_page_item(&mut self, item: &'a Item) {
        let Some(module) = item.module.clone() else {
            self.warnings
                .push(format!("{}: no module, skipped", item.qualified_name));
            return;
        };
        if self.attached_ops.contains(&(item as *const Item as usize)) {
            return;
        }
        let kind = match item.kind {
            ItemKind::Class | ItemKind::Struct => PageKind::Record,
            ItemKind::Enum => PageKind::Enum,
            // Namespace-scope Enumerators are constants hoisted out of an
            // unnamed enum; they page like variables, as do macros.
            ItemKind::Function
            | ItemKind::TypeAlias
            | ItemKind::Variable
            | ItemKind::Enumerator
            | ItemKind::Macro => PageKind::Leaf,
            _ => return,
        };
        let url = page_url(
            self.prefix,
            item.kind,
            &module,
            &item.qualified_name,
            self.root_ns,
        );
        let page = self.pages.entry(url.clone()).or_insert_with(|| Page {
            url,
            kind,
            title: short_name(&item.qualified_name, self.root_ns).to_owned(),
            module,
            items: Vec::new(),
        });
        page.items.push(item);

        // Nested records and enums get their own pages too.
        if kind == PageKind::Record {
            for child in &item.children {
                if matches!(
                    child.kind,
                    ItemKind::Class | ItemKind::Struct | ItemKind::Enum
                ) && child.access != Some(Access::Private)
                {
                    self.add_page_item(child);
                }
            }
        }
    }

    pub fn write(
        &self,
        env: &Environment,
        out_dir: &Path,
        search: &mut Vec<SearchEntry>,
        link: &LinkCtx,
    ) -> Result<usize> {
        use rayon::prelude::*;

        // Pages are independent read-only renders; search entries are
        // collected per page and flattened in page order, so the output
        // (index included) is byte-identical to a sequential build.
        let pages: Vec<&Page> = self.pages.values().collect();
        let per_page: Vec<Vec<SearchEntry>> = pages
            .par_iter()
            .map(|page| {
                let mut entries = Vec::new();
                let html = self.render_page(env, page, &mut entries, link)?;
                write_file(out_dir, &page.url, &html)?;
                Ok(entries)
            })
            .collect::<Result<_>>()?;
        for entries in per_page {
            search.extend(entries);
        }
        let mut pages_written = pages.len();

        pages_written += self.write_module_pages(env, out_dir)?;
        pages_written += self.write_source_pages(env, out_dir)?;
        pages_written += self.write_tree_index(env, out_dir)?;

        // The shared sidebar fragment (script-tag injected, like the
        // search index, so file:// works).
        std::fs::write(
            out_dir.join(self.prefix).join("nav.js"),
            format!(
                "window.UDG_NAV = {};\n",
                serde_json::to_string(&self.nav_data())?
            ),
        )?;

        Ok(pages_written)
    }

    // ----- shared helpers -------------------------------------------------

    fn src_link(&self, item: &Item) -> Option<String> {
        let src = item.source.as_ref()?;
        let canon = PathBuf::from(&src.file).canonicalize().ok()?;
        Some(format!(
            "{}#L{}",
            self.source_urls.get(&canon)?,
            src.line_start
        ))
    }

    fn docs_or_deprecation(&self, item: &Item, res: Option<Resolver>) -> Option<DocsView> {
        match &item.docs {
            Some(d) => Some(docs_view(d, item.deprecated.as_deref(), &self.md, res)),
            None => item.deprecated.as_deref().and_then(|m| {
                (!m.is_empty()).then(|| DocsView {
                    deprecated_html: Some(self.md.inline_with(m, res)),
                    ..DocsView::default()
                })
            }),
        }
    }

    fn summary_html(&self, item: &Item) -> String {
        item.docs
            .as_ref()
            .and_then(|d| d.summary.as_deref())
            .map(|s| self.md.inline(s))
            .unwrap_or_default()
    }

    fn member_view(
        &self,
        item: &Item,
        sig_chunks: Vec<sig::Chunk>,
        anchors: &mut HashMap<String, u32>,
        base: String,
        res: Option<Resolver>,
        sig_res: Option<Resolver>,
    ) -> MemberView {
        let n = anchors.entry(base.clone()).or_insert(0);
        *n += 1;
        let anchor = if *n == 1 { base } else { format!("{base}.{n}") };
        MemberView {
            anchor,
            sig_html: crate::render::linkify_chunks(&sig_chunks, sig_res),
            since: item.since.clone(),
            deprecated: item.deprecated.is_some(),
            docs: self.docs_or_deprecation(item, res),
            src_url: self.src_link(item),
        }
    }

    fn crumbs_for_module(&self, module: &str) -> Vec<Crumb> {
        let mut crumbs = vec![Crumb {
            name: self.title.into(),
            url: format!("{}/index.html", self.prefix),
        }];
        let mut path = String::new();
        for part in module.split('/') {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            crumbs.push(Crumb {
                name: self
                    .module_names
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| part.to_owned()),
                url: format!("{}/{path}/index.html", self.prefix),
            });
        }
        crumbs
    }

    // ----- item pages -----------------------------------------------------

    /// "Non-member operators" section for a record or enum page, with
    /// type-qualified search entries. Anchor assignment must mirror
    /// plan_urls exactly.
    fn related_ops_section(
        &self,
        page: &Page<'a>,
        item: &Item,
        anchors: &mut HashMap<String, u32>,
        search: &mut Vec<SearchEntry>,
        res: Option<Resolver>,
        sig_res: Option<Resolver>,
    ) -> Option<MemberSection> {
        let ops = self.related_ops.get(&item.id)?;
        let mut members = Vec::new();
        let mut seen_sigs = std::collections::HashSet::new();
        for op in ops {
            if !seen_sigs.insert(sig::for_function(op)) {
                continue;
            }
            let v = self.member_view(
                op,
                sig::for_function_chunks(op),
                anchors,
                format!("nm.{}", sanitize(&op.name)),
                res,
                sig_res,
            );
            search.push(SearchEntry {
                n: op.name.clone(),
                q: format!(
                    "{} ({})",
                    op.name,
                    short_name(&item.qualified_name, self.root_ns)
                ),
                k: "fn".into(),
                u: format!("{}#{}", page.url, v.anchor),
                m: format!("{}/{}", self.prefix, page.module),
            });
            members.push(v);
        }
        (!members.is_empty()).then(|| MemberSection {
            title: "Non-member operators".into(),
            members,
        })
    }

    fn binding_links(&self, item: &Item, link: &LinkCtx) -> Vec<BindingLink> {
        let mut out = Vec::new();
        for id in &item.bindings {
            let Some(url) = link.id_url.get(id) else {
                continue;
            };
            let (lang, rest) = id.split_once(':').unwrap_or(("", id));
            let label = match lang {
                "cpp" => "C++",
                "c" => "C",
                "py" => "Python",
                _ => lang,
            };
            out.push(BindingLink {
                label: format!("{label}: {rest}"),
                url: url.clone(),
            });
        }
        for fam in link.families.iter().filter(|f| f.owner_id == item.id) {
            out.push(BindingLink {
                label: format!("C: {}* ({} functions)", fam.prefix, fam.count),
                url: format!("c/index.html?q={}", fam.prefix),
            });
        }
        out
    }

    fn render_page(
        &self,
        env: &Environment,
        page: &Page<'a>,
        search: &mut Vec<SearchEntry>,
        link: &LinkCtx,
    ) -> Result<String> {
        let item = page.items[0];
        search.push(SearchEntry {
            n: item.name.clone(),
            q: short_name(&item.qualified_name, self.root_ns).to_owned(),
            k: kind_chip(item.kind).into(),
            u: page.url.clone(),
            m: format!("{}/{}", self.prefix, page.module),
        });

        let root = root_prefix(&page.url);
        let scope = item.qualified_name.clone();
        let resolve = |name: &str| -> Option<String> {
            match link.symbols.resolve(name, Some(&scope), Some(item.lang)) {
                Resolution::Unique(id) => link.id_url.get(&id).map(|u| format!("{root}{u}")),
                _ => None,
            }
        };
        let res: Option<Resolver> = Some(&resolve);
        let sig_resolve = |name: &str| -> Option<String> {
            match link.symbols.resolve(name, None, Some(item.lang)) {
                Resolution::Unique(id) => {
                    let u = link.id_url.get(&id)?;
                    // A type linking to the page it is on is noise.
                    if u.split('#').next() == Some(page.url.as_str()) {
                        return None;
                    }
                    Some(format!("{root}{u}"))
                }
                _ => None,
            }
        };
        let sig_res: Option<Resolver> = Some(&sig_resolve);
        let bindings = self.binding_links(item, link);
        let mut crumbs = self.crumbs_for_module(&page.module);
        crumbs.push(Crumb {
            name: page.title.clone(),
            url: String::new(),
        });
        let base = context! {
            project => self.project,
            title => page.title,
            root => root,
            nav_src => format!("{root}{}/nav.js", self.prefix),
            nav_current => format!("{}/{}/index.html", self.prefix, page.module),
            crumbs => crumbs,
        };

        let derived_rows: Vec<Row> = link
            .derived
            .get(&item.id)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| {
                        Some(Row {
                            name: e.display.clone(),
                            url: link.id_url.get(&e.id)?.clone(),
                            summary_html: e
                                .summary
                                .as_deref()
                                .map(|s| self.md.inline(s))
                                .unwrap_or_default(),
                            since: None,
                            deprecated: false,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let defined_in = item.source.as_ref().and_then(|src| {
            let base = std::path::Path::new(&src.file)
                .file_name()?
                .to_string_lossy()
                .into_owned();
            let name = match self.include_prefix {
                Some(p) => format!("<{p}/{base}>"),
                None => base,
            };
            Some(context! { name => name, url => self.src_link(item) })
        });
        let bindings_ctx =
            context! { bindings => bindings, derived => derived_rows, defined_in => defined_in };
        let html = match page.kind {
            PageKind::Record => {
                let view = self.record_view(page, search, res, sig_res, link)?;
                env.get_template("record.html")?
                    .render(context! { ..base, ..bindings_ctx, ..view })?
            }
            PageKind::Enum => {
                let view = self.enum_view(page, search, res, sig_res)?;
                env.get_template("enum.html")?
                    .render(context! { ..base, ..bindings_ctx, ..view })?
            }
            PageKind::Leaf => {
                let view = self.leaf_view(page, res, sig_res)?;
                env.get_template("leaf.html")?
                    .render(context! { ..base, ..bindings_ctx, ..view })?
            }
        };
        Ok(html)
    }

    fn record_view(
        &self,
        page: &Page<'a>,
        search: &mut Vec<SearchEntry>,
        res: Option<Resolver>,
        sig_res: Option<Resolver>,
        link: &LinkCtx,
    ) -> Result<minijinja::Value> {
        let item = page.items[0];
        let mut anchors: HashMap<String, u32> = HashMap::new();
        let mut sections: Vec<MemberSection> = Vec::new();

        let push_section =
            |title: &str, members: Vec<MemberView>, sections: &mut Vec<MemberSection>| {
                if !members.is_empty() {
                    sections.push(MemberSection {
                        title: title.into(),
                        members,
                    });
                }
            };

        let mut ctors = Vec::new();
        let mut methods = Vec::new();
        let mut prot_methods = Vec::new();
        let mut fields = Vec::new();
        let mut aliases = Vec::new();
        let mut nested: Vec<Row> = Vec::new();
        let mut constants: Vec<VariantView> = Vec::new();

        for child in &item.children {
            if child.access == Some(Access::Private) {
                continue;
            }
            let protected = child.access == Some(Access::Protected);
            match child.kind {
                ItemKind::Constructor => {
                    let v = self.member_view(
                        child,
                        sig::for_function_chunks(child),
                        &mut anchors,
                        format!("m.{}", sanitize(&child.name)),
                        res,
                        sig_res,
                    );
                    self.push_member_search(search, page, child, &v);
                    ctors.push(v);
                }
                ItemKind::Destructor if (child.docs.is_some() || child.deprecated.is_some()) => {
                    let v = self.member_view(
                        child,
                        sig::for_function_chunks(child),
                        &mut anchors,
                        format!("m.{}", sanitize(&child.name)),
                        res,
                        sig_res,
                    );
                    ctors.push(v);
                }
                ItemKind::Method | ItemKind::ConversionFunction => {
                    let v = self.member_view(
                        child,
                        sig::for_function_chunks(child),
                        &mut anchors,
                        format!("m.{}", sanitize(&child.name)),
                        res,
                        sig_res,
                    );
                    self.push_member_search(search, page, child, &v);
                    if protected {
                        prot_methods.push(v);
                    } else {
                        methods.push(v);
                    }
                }
                ItemKind::Field | ItemKind::Variable => {
                    let v = self.member_view(
                        child,
                        sig::for_leaf_chunks(child),
                        &mut anchors,
                        format!("f.{}", sanitize(&child.name)),
                        res,
                        sig_res,
                    );
                    self.push_member_search(search, page, child, &v);
                    fields.push(v);
                }
                ItemKind::TypeAlias => {
                    let v = self.member_view(
                        child,
                        sig::for_leaf_chunks(child),
                        &mut anchors,
                        format!("t.{}", sanitize(&child.name)),
                        res,
                        sig_res,
                    );
                    aliases.push(v);
                }
                // Constants hoisted out of an unnamed enum render as a
                // value table, like enumerators on an enum page.
                ItemKind::Enumerator => {
                    let anchor = format!("e.{}", sanitize(&child.name));
                    search.push(SearchEntry {
                        n: child.name.clone(),
                        q: short_name(&child.qualified_name, self.root_ns).to_owned(),
                        k: "const".into(),
                        u: format!("{}#{anchor}", page.url),
                        m: format!("{}/{}", self.prefix, page.module),
                    });
                    constants.push(self.variant_view(child, anchor));
                }
                ItemKind::Class | ItemKind::Struct | ItemKind::Enum => {
                    nested.push(Row {
                        name: child.name.clone(),
                        url: page_url(
                            self.prefix,
                            child.kind,
                            &page.module,
                            &child.qualified_name,
                            self.root_ns,
                        ),
                        summary_html: self.summary_html(child),
                        since: child.since.clone(),
                        deprecated: child.deprecated.is_some(),
                    });
                }
                _ => {}
            }
        }

        push_section("Constructors", ctors, &mut sections);
        push_section("Methods", methods, &mut sections);
        push_section("Protected methods", prot_methods, &mut sections);
        push_section("Member types", aliases, &mut sections);
        push_section("Members", fields, &mut sections);
        if let Some(sec) = self.related_ops_section(page, item, &mut anchors, search, res, sig_res)
        {
            sections.push(sec);
        }

        // The full surface a derived type offers: public members of the
        // base chain, grouped by the base that provides them,
        // derived-first so a name shown once is the one C++ lookup
        // finds. Names the class declares itself (overrides) shadow the
        // rest; constructors and `operator=` are not inherited.
        let mut inherited: Vec<InheritedGroup> = Vec::new();
        {
            let mut seen: std::collections::HashSet<String> =
                item.children.iter().map(|c| c.name.clone()).collect();
            let mut queue: std::collections::VecDeque<String> =
                item.bases.iter().cloned().collect();
            let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
            while let Some(base_name) = queue.pop_front() {
                let stripped = base_name.split('<').next().unwrap_or(&base_name).trim();
                let Resolution::Unique(base_id) =
                    link.symbols.resolve(stripped, None, Some(item.lang))
                else {
                    continue;
                };
                if !visited.insert(base_id.clone()) {
                    continue;
                }
                let Some(base_item) = link.items_by_id.get(&base_id) else {
                    continue;
                };
                let mut members: Vec<InheritedMember> = Vec::new();
                for c in &base_item.children {
                    if c.access != Some(Access::Public)
                        || !matches!(
                            c.kind,
                            ItemKind::Method
                                | ItemKind::ConversionFunction
                                | ItemKind::Field
                                | ItemKind::Variable
                                | ItemKind::TypeAlias
                                | ItemKind::Enumerator
                        )
                        || c.name == "operator="
                        || !seen.insert(c.name.clone())
                    {
                        continue;
                    }
                    members.push(InheritedMember {
                        name: c.name.clone(),
                        url: link.id_url.get(&c.id).cloned(),
                    });
                }
                if !members.is_empty() {
                    inherited.push(InheritedGroup {
                        name: short_name(&base_item.qualified_name, self.root_ns).to_owned(),
                        url: link.id_url.get(&base_id).cloned(),
                        members,
                    });
                }
                queue.extend(base_item.bases.iter().cloned());
            }
        }

        Ok(minijinja::Value::from_serialize(&SerializableRecord {
            kind_word: if item.kind == ItemKind::Struct {
                "Struct"
            } else {
                "Class"
            },
            sig_html: crate::render::linkify_chunks(&sig::for_record_chunks(item), sig_res),
            since: item.since.clone(),
            deprecated: item.deprecated.is_some(),
            docs: self.docs_or_deprecation(item, res),
            item_src: self.src_link(item),
            member_sections: sections,
            nested,
            constants,
            inherited,
        }))
    }

    fn variant_view(&self, child: &Item, anchor: String) -> VariantView {
        VariantView {
            anchor,
            name: child.name.clone(),
            value: child.value.clone(),
            deprecated: child.deprecated.is_some(),
            dep_html: child
                .deprecated
                .as_deref()
                .filter(|m| !m.is_empty())
                .map(|m| self.md.inline(m)),
            summary_html: child
                .docs
                .as_ref()
                .and_then(|d| d.summary.as_deref())
                .map(|s| self.md.inline(s)),
        }
    }

    fn push_member_search(
        &self,
        search: &mut Vec<SearchEntry>,
        page: &Page<'a>,
        child: &Item,
        view: &MemberView,
    ) {
        search.push(SearchEntry {
            n: child.name.clone(),
            q: short_name(&child.qualified_name, self.root_ns).to_owned(),
            k: kind_chip(child.kind).into(),
            u: format!("{}#{}", page.url, view.anchor),
            m: format!("{}/{}", self.prefix, page.module),
        });
    }

    fn enum_view(
        &self,
        page: &Page<'a>,
        search: &mut Vec<SearchEntry>,
        res: Option<Resolver>,
        sig_res: Option<Resolver>,
    ) -> Result<minijinja::Value> {
        let item = page.items[0];
        let mut variants = Vec::new();
        for child in &item.children {
            let anchor = format!("e.{}", sanitize(&child.name));
            search.push(SearchEntry {
                n: child.name.clone(),
                q: short_name(&child.qualified_name, self.root_ns).to_owned(),
                k: "variant".into(),
                u: format!("{}#{anchor}", page.url),
                m: format!("{}/{}", self.prefix, page.module),
            });
            variants.push(self.variant_view(child, anchor));
        }
        let mut sig_chunks = vec![sig::Chunk {
            text: format!("enum {}", short_name(&item.qualified_name, self.root_ns)),
            ty: false,
        }];
        if let Some(u) = &item.ty {
            sig_chunks.push(sig::Chunk {
                text: " : ".into(),
                ty: false,
            });
            sig_chunks.push(sig::Chunk {
                text: u.clone(),
                ty: true,
            });
        }
        let mut anchors: HashMap<String, u32> = HashMap::new();
        let member_sections: Vec<MemberSection> = self
            .related_ops_section(page, item, &mut anchors, search, res, sig_res)
            .into_iter()
            .collect();
        Ok(minijinja::Value::from_serialize(&SerializableEnum {
            sig_html: crate::render::linkify_chunks(&sig_chunks, sig_res),
            member_sections,
            since: item.since.clone(),
            deprecated: item.deprecated.is_some(),
            docs: self.docs_or_deprecation(item, res),
            item_src: self.src_link(item),
            variants,
        }))
    }

    fn leaf_view(
        &self,
        page: &Page<'a>,
        res: Option<Resolver>,
        sig_res: Option<Resolver>,
    ) -> Result<minijinja::Value> {
        let kind_word = match page.items[0].kind {
            ItemKind::Function => "Function",
            ItemKind::TypeAlias => "Type alias",
            ItemKind::Macro => "Macro",
            _ => "Constant",
        };
        let mut anchors = HashMap::new();
        let mut entries: Vec<MemberView> = Vec::new();
        let mut seen_sigs = std::collections::HashSet::new();
        for (i, item) in page.items.iter().enumerate() {
            // Re-declarations of the same function collapse to one entry.
            if !seen_sigs.insert(sig::for_leaf(item)) {
                continue;
            }
            let v = self.member_view(
                item,
                sig::for_leaf_chunks(item),
                &mut anchors,
                format!("o.{}", i + 1),
                res,
                sig_res,
            );
            entries.push(v);
        }
        Ok(minijinja::Value::from_serialize(&SerializableLeaf {
            kind_word,
            entries,
        }))
    }

    // ----- module / index / source pages ----------------------------------

    fn module_children(&self) -> HashMap<Option<String>, Vec<&Module>> {
        let mut map: HashMap<Option<String>, Vec<&Module>> = HashMap::new();
        for m in self.modules {
            map.entry(m.parent.clone()).or_default().push(m);
        }
        for v in map.values_mut() {
            v.sort_by_key(|a| a.name.to_lowercase());
        }
        map
    }

    fn write_module_pages(&self, env: &Environment, out_dir: &Path) -> Result<usize> {
        let children = self.module_children();
        let mut written = 0;
        for m in self.modules {
            let url = format!("{}/{}/index.html", self.prefix, m.id);
            let root = root_prefix(&url);

            let submodules: Vec<Row> = children
                .get(&Some(m.id.clone()))
                .map(|kids| {
                    kids.iter()
                        .map(|k| Row {
                            name: k.name.clone(),
                            url: format!("{}/{}/index.html", self.prefix, k.id),
                            summary_html: k
                                .brief
                                .as_deref()
                                .map(|b| self.md.inline(b))
                                .unwrap_or_default(),
                            since: None,
                            deprecated: false,
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut sections: Vec<Section> = Vec::new();
            let group = |title: &str, kinds: &[PageKind], want: &[&str]| -> Section {
                let mut rows: Vec<Row> = self
                    .pages
                    .values()
                    .filter(|p| p.module == m.id && kinds.contains(&p.kind))
                    .filter(|p| want.is_empty() || want.contains(&kind_prefix(p.items[0].kind)))
                    .map(|p| Row {
                        name: p.title.clone(),
                        url: p.url.clone(),
                        summary_html: self.summary_html(p.items[0]),
                        since: p.items[0].since.clone(),
                        deprecated: p.items[0].deprecated.is_some(),
                    })
                    .collect();
                rows.sort_by(|a, b| a.name.cmp(&b.name));
                Section {
                    title: title.into(),
                    rows,
                }
            };
            for sec in [
                group("Classes", &[PageKind::Record], &[]),
                group("Enums", &[PageKind::Enum], &[]),
                group("Functions", &[PageKind::Leaf], &["fn"]),
                group("Type aliases", &[PageKind::Leaf], &["type"]),
                group("Constants", &[PageKind::Leaf], &["const"]),
                group("Macros", &[PageKind::Leaf], &["macro"]),
            ] {
                if !sec.rows.is_empty() {
                    sections.push(sec);
                }
            }

            let mut crumbs = self.crumbs_for_module(&m.id);
            crumbs.last_mut().expect("nonempty crumbs").url = String::new();

            let html = env.get_template("module.html")?.render(context! {
                project => self.project,
                title => m.name,
                root => root,
                nav_src => format!("{root}{}/nav.js", self.prefix),
                nav_current => format!("{}/{}/index.html", self.prefix, m.id),
                crumbs => crumbs,
                brief_html => m.brief.as_deref().map(|b| self.md.inline(b)),
                submodules => submodules,
                sections => sections,
            })?;
            write_file(out_dir, &url, &html)?;
            written += 1;
        }
        Ok(written)
    }

    fn write_source_pages(&self, env: &Environment, out_dir: &Path) -> Result<usize> {
        use rayon::prelude::*;

        // Syntect highlighting makes these the most expensive pages;
        // each is independent.
        let jobs: Vec<(&Module, &String)> = self
            .modules
            .iter()
            .flat_map(|m| m.headers.iter().map(move |h| (m, h)))
            .collect();
        let written = jobs
            .par_iter()
            .map(|(m, header)| {
                let Ok(canon) = PathBuf::from(header).canonicalize() else {
                    return Ok(0);
                };
                let Ok(text) = std::fs::read_to_string(&canon) else {
                    return Ok(0);
                };
                let Some(url) = self.source_urls.get(&canon).cloned() else {
                    return Ok(0);
                };
                // A header shared by two modules renders once, at its
                // planned URL.
                if !url.starts_with(&format!("{}/{}/", self.prefix, m.id)) {
                    return Ok(0);
                }
                let root = root_prefix(&url);
                let ext = canon.extension().and_then(|e| e.to_str()).unwrap_or("");
                // One flowing highlight stream (spans cross lines
                // freely) beside a separate line-number gutter: no
                // per-line wrappers, no re-opened spans on every line —
                // source views were more than half the site's bytes.
                let code = crate::render::highlight::block(&text, ext)
                    .unwrap_or_else(|| html_escape(&text));
                let mut linenos = String::with_capacity(text.lines().count() * 24);
                for n in 1..=text.lines().count() {
                    // display:block anchors stack one per line on their own; a
                    // newline here would add a second, drifting the gutter.
                    linenos.push_str(&format!("<a id=\"L{n}\" href=\"#L{n}\">{n}</a>"));
                }
                let base = canon
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut crumbs = self.crumbs_for_module(&m.id);
                crumbs.push(Crumb {
                    name: base.clone(),
                    url: String::new(),
                });
                let html = env.get_template("source.html")?.render(context! {
                    project => self.project,
                    title => base,
                    root => root,
                    nav_src => format!("{root}{}/nav.js", self.prefix),
                    nav_current => format!("{}/{}/index.html", self.prefix, m.id),
                    crumbs => crumbs,
                    linenos_html => linenos,
                    code_html => code,
                })?;
                write_file(out_dir, &url, &html)?;
                Ok(1)
            })
            .collect::<Result<Vec<usize>>>()?
            .iter()
            .sum();
        Ok(written)
    }

    fn write_tree_index(&self, env: &Environment, out_dir: &Path) -> Result<usize> {
        let url = format!("{}/index.html", self.prefix);
        let root = root_prefix(&url);
        let html = env.get_template("index.html")?.render(context! {
            project => self.project,
            title => format!("{} {}", self.project, self.title),
            root => root,
            nav_src => format!("{root}{}/nav.js", self.prefix),
            crumbs => Vec::<Crumb>::new(),
            stats => self.stats_line(),
            tree_html => self.module_tree_html(&root),
        })?;
        write_file(out_dir, &url, &html)?;
        Ok(1)
    }

    // ----- navigation -----------------------------------------------------

    /// The tree's sidebar nav as one shared fragment: site-root-relative
    /// hrefs, no current/open state. Pages reference it via `nav.js`
    /// and a loader in search.js prefixes each page's root and marks
    /// the current module — inlining the tree into every page was most
    /// of a small page's bytes.
    fn nav_data(&self) -> String {
        let children = self.module_children();
        let mut out = format!(
            "<div class=\"navtitle\"><a href=\"{}/index.html\">{}</a></div><ul class=\"navtree\">",
            self.prefix,
            html_escape(self.title)
        );
        for m in children.get(&None).into_iter().flatten() {
            self.nav_node(m, &children, &mut out);
        }
        out.push_str("</ul>");
        if self.others.len() > 1 {
            out.push_str("<div class=\"navtitle other\">Other APIs</div><ul class=\"navtree\">");
            for (title, prefix) in self.others {
                if prefix != self.prefix {
                    out.push_str(&format!(
                        "<li><a href=\"{prefix}/index.html\">{}</a></li>",
                        html_escape(title)
                    ));
                }
            }
            out.push_str("</ul>");
        }
        out
    }

    fn nav_node(
        &self,
        m: &Module,
        children: &HashMap<Option<String>, Vec<&Module>>,
        out: &mut String,
    ) {
        out.push_str(&format!(
            "<li><a href=\"{}/{}/index.html\">{}</a>",
            self.prefix,
            m.id,
            html_escape(&m.name)
        ));
        if let Some(kids) = children.get(&Some(m.id.clone())) {
            out.push_str("<ul>");
            for k in kids {
                self.nav_node(k, children, out);
            }
            out.push_str("</ul>");
        }
        out.push_str("</li>");
    }

    pub fn module_tree_html(&self, root: &str) -> String {
        let children = self.module_children();
        let mut out = String::from("<ul class=\"modtree\">");
        fn node(
            site: &Site,
            m: &Module,
            children: &HashMap<Option<String>, Vec<&Module>>,
            root: &str,
            out: &mut String,
        ) {
            out.push_str(&format!(
                "<li><a href=\"{root}{}/{}/index.html\">{}</a>",
                site.prefix,
                m.id,
                html_escape(&m.name)
            ));
            if let Some(brief) = &m.brief {
                out.push_str(&format!(
                    " <span class=\"brief\">— {}</span>",
                    site.md.inline(brief)
                ));
            }
            if let Some(kids) = children.get(&Some(m.id.clone())) {
                out.push_str("<ul>");
                for k in kids {
                    node(site, k, children, root, out);
                }
                out.push_str("</ul>");
            }
            out.push_str("</li>");
        }
        for m in children.get(&None).into_iter().flatten() {
            node(self, m, &children, root, &mut out);
        }
        out.push_str("</ul>");
        out
    }
}

#[derive(Serialize)]
struct SerializableRecord {
    kind_word: &'static str,
    sig_html: String,
    since: Option<String>,
    deprecated: bool,
    docs: Option<DocsView>,
    item_src: Option<String>,
    member_sections: Vec<MemberSection>,
    nested: Vec<Row>,
    constants: Vec<VariantView>,
    inherited: Vec<InheritedGroup>,
}

#[derive(Serialize)]
struct InheritedGroup {
    name: String,
    url: Option<String>,
    members: Vec<InheritedMember>,
}

#[derive(Serialize)]
struct InheritedMember {
    name: String,
    url: Option<String>,
}

#[derive(Serialize)]
struct SerializableEnum {
    sig_html: String,
    member_sections: Vec<MemberSection>,
    since: Option<String>,
    deprecated: bool,
    docs: Option<DocsView>,
    item_src: Option<String>,
    variants: Vec<VariantView>,
}

#[derive(Serialize)]
struct SerializableLeaf {
    kind_word: &'static str,
    entries: Vec<MemberView>,
}

/// Depth-first traversal preserving the arena lifetime (Item::walk's
/// closure only sees a reborrow, which cannot be stored).
fn walk_items<'a>(item: &'a Item, f: &mut impl FnMut(&'a Item)) {
    f(item);
    for c in &item.children {
        walk_items(c, f);
    }
}

/// Identifier paths (`Foo`, `Botan::Foo`) appearing in a type string.
fn ident_tokens(ty: &str) -> Vec<String> {
    let chars: Vec<char> = ty.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            loop {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                if i + 2 < chars.len()
                    && chars[i] == ':'
                    && chars[i + 1] == ':'
                    && (chars[i + 2].is_ascii_alphabetic() || chars[i + 2] == '_')
                {
                    i += 2;
                } else {
                    break;
                }
            }
            out.push(chars[start..i].iter().collect());
        } else {
            i += 1;
        }
    }
    out
}

pub(crate) fn write_file(out_dir: &Path, url: &str, html: &str) -> Result<()> {
    let html = relativize_links(html, url);
    let path = out_dir.join(url);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    std::fs::write(&path, html).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Shrink internal links: everything is generated site-root-relative
/// (`{root}cpp/x/y.html`), so a link to a same-directory file costs a
/// climb up and back down — and minijinja spells each `/` in attributes
/// as 6-byte `&#x2f;`. Rewrite every href/src that starts with this
/// page's root prefix to the shortest path from the page's directory,
/// with plain slashes (attribute-safe). External URLs, fragments, and
/// already-relative links pass through untouched.
fn relativize_links(html: &str, page_url: &str) -> String {
    let page_dirs: Vec<&str> = match page_url.rsplit_once('/') {
        Some((dir, _)) => dir.split('/').collect(),
        None => return html.to_owned(),
    };
    let root: String = "../".repeat(page_dirs.len());

    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        // Whichever attribute comes first. Escaped text can't contain a
        // raw `"` (html_escape emits &quot;), so these only match real
        // attributes.
        let (pos, attr_len) = match (rest.find("href=\""), rest.find("src=\"")) {
            (None, None) => break,
            (Some(h), None) => (h, "href=\"".len()),
            (None, Some(s)) => (s, "src=\"".len()),
            (Some(h), Some(s)) if h < s => (h, "href=\"".len()),
            (_, Some(s)) => (s, "src=\"".len()),
        };
        let val_start = pos + attr_len;
        let Some(val_len) = rest[val_start..].find('"') else {
            break;
        };
        out.push_str(&rest[..val_start]);
        let val = rest[val_start..val_start + val_len].replace("&#x2f;", "/");
        out.push_str(&shorten(&val, &root, &page_dirs));
        rest = &rest[val_start + val_len..];
    }
    out.push_str(rest);
    out
}

/// One attribute value: if it climbs exactly to the site root, drop the
/// directories it shares with the page and re-climb only the rest.
fn shorten(val: &str, root: &str, page_dirs: &[&str]) -> String {
    let Some(target) = val.strip_prefix(root) else {
        return val.to_owned();
    };
    if target.starts_with("../") || target.starts_with('#') || target.is_empty() {
        return val.to_owned();
    }
    let (path, frag) = match target.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (target, None),
    };
    let dirs: Vec<&str> = match path.rsplit_once('/') {
        Some((d, _)) => d.split('/').collect(),
        None => Vec::new(),
    };
    let common = page_dirs
        .iter()
        .zip(&dirs)
        .take_while(|(a, b)| a == b)
        .count();
    let mut short = "../".repeat(page_dirs.len() - common);
    let skip: usize = dirs.iter().take(common).map(|d| d.len() + 1).sum();
    short.push_str(&path[skip..]);
    if let Some(f) = frag {
        short.push('#');
        short.push_str(f);
    }
    short
}

#[cfg(test)]
mod tests {
    use super::{plan_source_urls, relativize_links};

    #[test]
    fn same_basename_headers_get_distinct_urls() {
        let dir = std::env::temp_dir().join(format!("udg-srcurl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["a", "b"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::write(dir.join(sub).join("config.h"), "x").unwrap();
        }
        let m = crate::ir::Module {
            id: "core".into(),
            name: "Core".into(),
            brief: None,
            parent: None,
            headers: vec![
                dir.join("a/config.h").display().to_string(),
                dir.join("b/config.h").display().to_string(),
            ],
        };
        let urls = plan_source_urls("cpp", std::slice::from_ref(&m));
        let a = &urls[&dir.join("a/config.h").canonicalize().unwrap()];
        let b = &urls[&dir.join("b/config.h").canonicalize().unwrap()];
        assert_eq!(a, "cpp/core/src.config.h.html");
        assert_eq!(b, "cpp/core/src.config.h.2.html");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_shorten_to_the_page_directory() {
        let page = "cpp/pubkey/ec_group/class.EC_Group.html";
        let go = |html: &str| relativize_links(html, page);

        // Same directory: no climb at all (entity-escaped input).
        assert_eq!(
            go(
                r#"<a href="..&#x2f;..&#x2f;..&#x2f;cpp&#x2f;pubkey&#x2f;ec_group&#x2f;src.ec_group.h.html#L42">x</a>"#
            ),
            r#"<a href="src.ec_group.h.html#L42">x</a>"#
        );
        // Sibling module: climb one, descend one.
        assert_eq!(
            go(r#"<a href="../../../cpp/pubkey/dl_group/class.DL_Group.html">x</a>"#),
            r#"<a href="../dl_group/class.DL_Group.html">x</a>"#
        );
        // Cross-tree: nothing shared, full climb kept.
        assert_eq!(
            go(r#"<a href="../../../py/botan3/class.PK.html">x</a>"#),
            r#"<a href="../../../py/botan3/class.PK.html">x</a>"#
        );
        // Fragments, external URLs, already-relative and script src.
        assert_eq!(
            go(r##"<a href="#m.encode">x</a>"##),
            r##"<a href="#m.encode">x</a>"##
        );
        assert_eq!(
            go(r#"<a href="https://example.com/a/b">x</a>"#),
            r#"<a href="https://example.com/a/b">x</a>"#
        );
        assert_eq!(
            go(r#"<script src="..&#x2f;..&#x2f;..&#x2f;cpp&#x2f;nav.js"></script>"#),
            // Shares the `cpp/` prefix with the page: two climbs, done.
            r#"<script src="../../nav.js"></script>"#
        );
        // Escaped text content is untouched.
        assert_eq!(
            go("<code>href=&quot;..&#x2f;x&quot;</code>"),
            "<code>href=&quot;..&#x2f;x&quot;</code>"
        );
    }
}
