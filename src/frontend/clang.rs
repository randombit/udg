//! The C/C++ frontend: parses configured public headers with libclang and
//! emits udg IR items.
//!
//! Project-specific macros (e.g. Botan's `BOTAN_PUBLIC_API(2,0)`) are
//! recovered by lexing the declaration's source tokens — macro invocations
//! are gone from the post-expansion AST but still present in the spelled
//! token stream — and mapped to IR semantics via configured
//! [`AttributeRule`]s.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::ir::{Access, DocBlock, Item, ItemKind, Language, Param, Signature, SourceSpan};
use anyhow::{Context, Result, anyhow};
use clang::{
    Accessibility, Clang, Entity, EntityKind, ExceptionSpecification, Index, TranslationUnit,
    Unsaved,
};

/// One project-attribute mapping rule, e.g.:
/// macro `BOTAN_PUBLIC_API`, args `["major", "minor"]`,
/// since-template `"{major}.{minor}"`.
#[derive(Debug, Clone)]
pub struct AttributeRule {
    pub macro_name: String,
    pub arg_names: Vec<String>,
    /// Template for `Item::since`, substituting `{argname}`.
    pub since: Option<String>,
    /// Template for `Item::deprecated`, substituting `{argname}`.
    pub deprecated: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CppConfig {
    /// Language standard, e.g. "c++20" or "c11".
    pub std: String,
    /// Parse as C rather than C++.
    pub c_mode: bool,
    /// Language tag for emitted items. Defaults from `c_mode`; set it
    /// explicitly to document a C API surface whose header must be
    /// parsed as C++ (e.g. Botan's ffi.h, which uses `[[deprecated]]`).
    pub language: Option<Language>,
    pub include_dirs: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub extra_args: Vec<String>,
    /// The public headers to document, already resolved to real files.
    pub headers: Vec<PathBuf>,
    /// Shipped-but-not-public headers (vendored specs like PKCS#11):
    /// their declaration and macro names feed reference resolution so
    /// xrefs to them are not rot, but nothing in them is documented.
    pub external_headers: Vec<PathBuf>,
    pub attributes: Vec<AttributeRule>,
}

pub struct ExtractOutput {
    pub items: Vec<Item>,
    /// Names (declarations and object-like macros) from external
    /// headers plus macros from documented headers: presence-only,
    /// for reference resolution.
    pub reference_names: Vec<String>,
    /// Rendered clang diagnostics of severity warning.
    pub diagnostics: Vec<String>,
    /// Error/fatal diagnostics: the AST is incomplete, so callers should
    /// treat these as fatal unless explicitly configured otherwise —
    /// coverage numbers over a partial AST are lies.
    pub errors: Vec<String>,
}

pub fn extract(cfg: &CppConfig) -> Result<ExtractOutput> {
    let clang = Clang::new().map_err(|e| anyhow!("failed to initialize libclang: {e}"))?;
    let index = Index::new(&clang, false, false);

    // One synthetic TU including every target header: each entity is seen
    // once, cross-header types resolve, and a single parse is fast enough.
    let mut tu_source = String::new();
    for h in &cfg.headers {
        let canonical = h
            .canonicalize()
            .with_context(|| format!("header not found: {}", h.display()))?;
        writeln!(tu_source, "#include \"{}\"", canonical.display())?;
    }

    let lang = cfg.language.unwrap_or(if cfg.c_mode {
        Language::C
    } else {
        Language::Cpp
    });
    let tu_path = if cfg.c_mode {
        "__udg_tu.c"
    } else {
        "__udg_tu.cpp"
    };
    let mut args: Vec<String> = vec![format!("-std={}", cfg.std)];
    // libclang does not reliably locate its builtin headers (stddef.h &
    // co.) on its own; point it at the driver's resource dir when we can.
    if !cfg.extra_args.iter().any(|a| a.contains("-resource-dir"))
        && let Some(dir) = detect_resource_dir()
    {
        args.push(format!("-resource-dir={dir}"));
    }
    for dir in &cfg.include_dirs {
        args.push(format!("-I{}", dir.display()));
    }
    for def in &cfg.defines {
        args.push(format!("-D{def}"));
    }
    args.extend(cfg.extra_args.iter().cloned());

    if std::env::var("UDG_DEBUG_ARGS").is_ok() {
        eprintln!("udg-debug args: {args:?}");
    }
    let tu = index
        .parser(tu_path)
        .arguments(&args)
        .unsaved(&[Unsaved::new(tu_path, &tu_source)])
        .skip_function_bodies(true)
        .detailed_preprocessing_record(true)
        .parse()
        .map_err(|e| anyhow!("libclang parse failed: {e}"))?;

    let (diagnostics, errors) = collect_diagnostics(&tu);

    let mut ctx = Ctx {
        lang,
        allowed: cfg
            .headers
            .iter()
            .filter_map(|h| h.canonicalize().ok())
            .collect(),
        external: cfg
            .external_headers
            .iter()
            .filter_map(|h| h.canonicalize().ok())
            .collect(),
        rules: &cfg.attributes,
        canon_cache: HashMap::new(),
        canon_display: HashMap::new(),
        sources: HashMap::new(),
    };

    let items = convert_scope_children(&tu.get_entity(), "", &mut ctx);

    // Macros in documented headers become items (C APIs live on
    // #defines); attribute-rule macros are markup, and undocumented
    // empty definitions (header guards, feature flags) stay
    // presence-only, as does everything from external headers.
    let mut reference_names: Vec<String> = Vec::new();
    let mut items = items;
    let mut seen_macros: HashSet<String> = HashSet::new();
    let mut macro_floors: HashMap<PathBuf, u32> = HashMap::new();
    for child in tu.get_entity().get_children() {
        match child.get_kind() {
            EntityKind::MacroDefinition => {
                let Some(name) = child.get_name() else {
                    continue;
                };
                if name.starts_with('_') {
                    continue;
                }
                let allowed = ctx.in_allowed_file(&child);
                if !allowed && !ctx.in_external_file(&child) {
                    continue;
                }
                let is_rule = ctx.rules.iter().any(|r| r.macro_name == name);
                if allowed
                    && !is_rule
                    && seen_macros.insert(name.clone())
                    && let Some(item) = convert_macro(&child, &name, &mut macro_floors, &mut ctx)
                {
                    items.push(item);
                    continue;
                }
                reference_names.push(name);
            }
            _ => {
                if ctx.in_external_file(&child) {
                    collect_names(&child, &mut reference_names);
                }
            }
        }
    }
    reference_names.sort();
    reference_names.dedup();

    Ok(ExtractOutput {
        items,
        reference_names,
        diagnostics,
        errors,
    })
}

/// A `#define` in a documented header as a real item: value text,
/// parameters for function-like macros, and docs from the directly
/// adjacent comment (libclang does not associate comments with
/// macros). Returns None for undocumented empty definitions — header
/// guards and feature flags are presence, not API.
fn convert_macro(
    e: &Entity,
    name: &str,
    floors: &mut HashMap<PathBuf, u32>,
    ctx: &mut Ctx,
) -> Option<Item> {
    let range = e.get_range()?;
    let start = range.get_start().get_file_location();
    let file = start.file?.get_path();
    let floor = floors.get(&file).copied().unwrap_or(0);
    floors.insert(file.clone(), range.get_end().get_file_location().offset);

    let spellings: Vec<String> = range.tokenize().iter().map(|t| t.get_spelling()).collect();
    // tokens: NAME [( params )] replacement...
    let mut idx = 1;
    let mut params: Vec<String> = Vec::new();
    if e.is_function_like_macro() && spellings.get(1).map(String::as_str) == Some("(") {
        idx = 2;
        while idx < spellings.len() && spellings[idx] != ")" {
            if spellings[idx] != "," {
                params.push(spellings[idx].clone());
            }
            idx += 1;
        }
        idx += 1;
    }
    let value = join_tokens(spellings[idx.min(spellings.len())..].iter().cloned());
    let docs = macro_doc(e, floor, ctx).map(|raw| crate::comments::parse(&raw));

    if value.is_empty() && docs.as_ref().is_none_or(|d| d.summary.is_none()) {
        return None;
    }

    let mut item = Item::new(ctx.lang, ItemKind::Macro, name, name);
    item.source = source_span(e, ctx);
    if !value.is_empty() {
        item.value = Some(truncate_value(&value));
    }
    if e.is_function_like_macro() {
        item.signature = Some(Signature {
            params: params
                .into_iter()
                .map(|p| Param {
                    name: Some(p),
                    ty: String::new(),
                    default: None,
                })
                .collect(),
            ..Signature::default()
        });
    }
    item.docs = docs.filter(|d| *d != DocBlock::default());
    Some(item)
}

fn truncate_value(s: &str) -> String {
    if s.len() <= 80 {
        return s.to_owned();
    }
    let cut = (0..=80).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}…", &s[..cut])
}

/// The doc comment for a macro: the trailing comment on the `#define`
/// line, or a doc-comment run ending directly above it (at most one
/// blank line between). Unlike declaration recovery, adjacency is
/// required — the floor window can span unrelated declarations.
fn macro_doc(e: &Entity, floor: u32, ctx: &mut Ctx) -> Option<String> {
    let range = e.get_range()?;
    let start = range.get_start().get_file_location();
    let end = range.get_end().get_file_location();
    let file = start.file?;
    let path = file.get_path();

    // Trailing form: rest of the #define's last line.
    if let Some(line_text) = ctx.window(&path, end.offset, end.offset + 200)
        && let Some(eol) = line_text.find('\n').or(Some(line_text.len()))
    {
        let tail = &line_text[..eol];
        for opener in ["/**<", "/**", "///<", "///"] {
            if let Some(p) = tail.find(opener) {
                return Some(tail[p..].trim().to_owned());
            }
        }
    }

    // Adjacent preceding run: scan the floor-bounded window, keep the
    // trailing doc-comment run only if it ends just above the #define.
    // The `#` of `#define` starts the extent; comments end before it.
    let scan_start = floor.min(start.offset);
    let w = ctx.window(&path, scan_start, start.offset)?.to_owned();
    // The macro's extent begins at its NAME, so the window ends with
    // the `#define ` fragment of its own line — drop that partial line
    // or it would sever the adjacency run.
    let w = &w[..w.rfind('\n').map(|p| p + 1).unwrap_or(0)];
    let mut run: Vec<&str> = Vec::new();
    let mut blank_since_run = usize::MAX;
    for line in w.lines() {
        let t = line.trim();
        let is_doc = ["/**", "/*!", "///", "//!"]
            .iter()
            .any(|p| t.starts_with(p))
            || (!run.is_empty() && (t.starts_with('*') || t.ends_with("*/")));
        if is_doc {
            if blank_since_run != usize::MAX && blank_since_run > 0 {
                run.clear();
            }
            run.push(line);
            blank_since_run = 0;
        } else if t.is_empty() {
            if blank_since_run != usize::MAX {
                blank_since_run += 1;
            }
        } else {
            run.clear();
            blank_since_run = usize::MAX;
        }
    }
    if run.is_empty() || blank_since_run > 1 {
        return None;
    }
    Some(run.join(
        "
",
    ))
}

/// Every named declaration under an external-header entity: types,
/// enumerators, functions, variables — names only.
fn collect_names(e: &Entity, out: &mut Vec<String>) {
    if let Some(name) = e.get_name()
        && !name.starts_with('_')
        && !name.starts_with("(unnamed")
    {
        out.push(name);
    }
    for c in e.get_children() {
        collect_names(&c, out);
    }
}

fn detect_resource_dir() -> Option<String> {
    let out = std::process::Command::new("clang")
        .arg("--print-resource-dir")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!dir.is_empty() && std::path::Path::new(&dir).exists()).then_some(dir)
}

/// (warnings, errors) — errors mean the AST is incomplete.
fn collect_diagnostics(tu: &TranslationUnit) -> (Vec<String>, Vec<String>) {
    use clang::diagnostic::Severity;
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    for d in tu.get_diagnostics() {
        let severity = d.get_severity();
        if severity < Severity::Warning {
            continue;
        }
        let loc = d.get_location().get_file_location();
        let file = loc
            .file
            .map(|f| f.get_path().display().to_string())
            .unwrap_or_else(|| "<unknown>".into());
        let text = format!("{:?}: {}:{}: {}", severity, file, loc.line, d.get_text());
        if severity >= Severity::Error {
            errors.push(text);
        } else {
            warnings.push(text);
        }
    }
    (warnings, errors)
}

struct Ctx<'a> {
    lang: Language,
    allowed: HashSet<PathBuf>,
    external: HashSet<PathBuf>,
    rules: &'a [AttributeRule],
    canon_cache: HashMap<PathBuf, bool>,
    canon_display: HashMap<PathBuf, String>,
    /// Raw file contents, for cheap "is a token scan worth it" checks:
    /// libclang tokenization per declaration is what backward scans
    /// spend their time on, and most windows contain neither a macro
    /// nor a comment.
    sources: HashMap<PathBuf, String>,
}

impl Ctx<'_> {
    /// Canonical display form of a source path. libclang registers a
    /// file under whichever spelling first opened it — the TU's direct
    /// (already canonical) include or a transitive `-I`-relative one
    /// through a symlink — so raw spellings vary with include order.
    fn canonical_display(&mut self, path: PathBuf) -> String {
        if let Some(hit) = self.canon_display.get(&path) {
            return hit.clone();
        }
        let display = path
            .canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string();
        self.canon_display.insert(path, display.clone());
        display
    }

    /// The spelled source between two byte offsets of a file. None when
    /// the file is unreadable or the offsets don't slice it cleanly —
    /// callers should fall back to the full token scan.
    fn window(&mut self, path: &Path, start: u32, end: u32) -> Option<&str> {
        if !self.sources.contains_key(path) {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            self.sources.insert(path.to_owned(), text);
        }
        let text = self.sources.get(path)?;
        text.get(start as usize..(end as usize).min(text.len()))
    }
    /// Is this entity declared in one of the external reference headers?
    fn in_external_file(&self, e: &Entity) -> bool {
        if self.external.is_empty() {
            return false;
        }
        e.get_location()
            .map(|l| l.get_file_location())
            .and_then(|l| l.file)
            .and_then(|f| f.get_path().canonicalize().ok())
            .is_some_and(|p| self.external.contains(&p))
    }

    /// Is this entity declared in one of the configured headers?
    fn in_allowed_file(&mut self, e: &Entity) -> bool {
        let Some(file) = e
            .get_location()
            .map(|l| l.get_file_location())
            .and_then(|l| l.file)
        else {
            return false;
        };
        let path = file.get_path();
        if let Some(&hit) = self.canon_cache.get(&path) {
            return hit;
        }
        let hit = path
            .canonicalize()
            .map(|c| self.allowed.contains(&c))
            .unwrap_or(false);
        self.canon_cache.insert(path, hit);
        hit
    }
}

/// Tracks, per source file, the end offset of the previously visited
/// sibling declaration. Leading attributes (spelled as project macros like
/// `BOTAN_DEPRECATED("...")`) fall *outside* a declaration's AST extent,
/// so the attribute scan must extend backward — but no further than the
/// previous sibling, or we would claim a neighbor's macros.
struct FloorTracker {
    by_file: HashMap<PathBuf, u32>,
    /// The containing entity's own file/name-offset: the initial floor for
    /// its first child (e.g. just past `class Widget`).
    container: Option<(PathBuf, u32)>,
}

impl FloorTracker {
    fn new(container: Option<&Entity>) -> Self {
        let container = container
            .and_then(|e| e.get_location())
            .map(|l| l.get_file_location())
            .and_then(|fl| Some((fl.file?.get_path(), fl.offset)));
        FloorTracker {
            by_file: HashMap::new(),
            container,
        }
    }

    fn floor_for(&self, e: &Entity) -> u32 {
        let Some(fl) = e.get_location().map(|l| l.get_file_location()) else {
            return 0;
        };
        let Some(path) = fl.file.map(|f| f.get_path()) else {
            return 0;
        };
        if let Some(&off) = self.by_file.get(&path) {
            return off;
        }
        match &self.container {
            Some((cpath, coff)) if *cpath == path => *coff,
            _ => 0,
        }
    }

    fn update(&mut self, e: &Entity) {
        let Some(range) = e.get_range() else { return };
        let end = range.get_end().get_file_location();
        if let Some(path) = end.file.map(|f| f.get_path()) {
            self.by_file.insert(path, end.offset);
        }
    }
}

/// Convert the children of a namespace-scope container: the TU itself, a
/// namespace, or a linkage spec (`extern "C" { ... }`), whose children
/// are spliced transparently into the surrounding scope.
fn convert_scope_children(e: &Entity, prefix: &str, ctx: &mut Ctx) -> Vec<Item> {
    let mut out = Vec::new();
    let container = (e.get_kind() != EntityKind::TranslationUnit).then_some(e);
    let mut floors = FloorTracker::new(container);
    for child in e.get_children() {
        let floor = floors.floor_for(&child);
        floors.update(&child);
        if !ctx.in_allowed_file(&child) {
            continue;
        }
        if child.get_kind() == EntityKind::LinkageSpec {
            out.extend(convert_scope_children(&child, prefix, ctx));
            continue;
        }
        if let Some(item) = convert(&child, prefix, floor, ctx) {
            if item.kind == ItemKind::Enum && item.name.is_empty() {
                out.extend(item.children);
            } else {
                out.push(item);
            }
        }
    }
    merge_namespaces(&mut out);
    out
}

fn convert(e: &Entity, prefix: &str, floor: u32, ctx: &mut Ctx) -> Option<Item> {
    let kind = match e.get_kind() {
        EntityKind::Namespace => ItemKind::Namespace,
        EntityKind::ClassDecl | EntityKind::ClassTemplate => ItemKind::Class,
        EntityKind::StructDecl => ItemKind::Struct,
        EntityKind::EnumDecl => ItemKind::Enum,
        EntityKind::EnumConstantDecl => ItemKind::Enumerator,
        EntityKind::FunctionDecl => ItemKind::Function,
        EntityKind::FunctionTemplate | EntityKind::Method => ItemKind::Method,
        EntityKind::Constructor => ItemKind::Constructor,
        EntityKind::Destructor => ItemKind::Destructor,
        EntityKind::ConversionFunction => ItemKind::ConversionFunction,
        EntityKind::TypedefDecl | EntityKind::TypeAliasDecl => ItemKind::TypeAlias,
        EntityKind::VarDecl => ItemKind::Variable,
        EntityKind::FieldDecl => ItemKind::Field,
        _ => return None,
    };
    // A FunctionTemplate cursor is used for both free and member function
    // templates; classify by whether we're inside a record. Constructor
    // and conversion templates (`template <typename... Args> Stream(...)`)
    // also arrive as FunctionTemplate — the templated declaration's kind
    // tells them apart.
    let kind = match (kind, e.get_semantic_parent().map(|p| p.get_kind())) {
        (ItemKind::Method, Some(EntityKind::Namespace) | Some(EntityKind::TranslationUnit)) => {
            ItemKind::Function
        }
        (ItemKind::Method, _) if e.get_kind() == EntityKind::FunctionTemplate => {
            match e.get_template_kind() {
                Some(EntityKind::Constructor) => ItemKind::Constructor,
                Some(EntityKind::ConversionFunction) => ItemKind::ConversionFunction,
                _ => ItemKind::Method,
            }
        }
        (k, _) => k,
    };

    // Records and enums: only document the definition, not forward decls.
    if matches!(kind, ItemKind::Class | ItemKind::Struct | ItemKind::Enum) && !e.is_definition() {
        return None;
    }

    // A deleted function is not API — it exists to poison overload
    // resolution.
    if matches!(
        kind,
        ItemKind::Function
            | ItemKind::Method
            | ItemKind::Constructor
            | ItemKind::Destructor
            | ItemKind::ConversionFunction
    ) && is_deleted_function(e, ctx)
    {
        return None;
    }

    // An unnamed enum (`enum { BLOCK_SIZE = 16 };`, the pre-constexpr
    // constant idiom) injects its enumerators into the enclosing scope in
    // C++; model that directly by scoping them to the parent and returning
    // a nameless wrapper for callers to splice. A typedef'd unnamed enum
    // is not anonymous to libclang — it takes the typedef's name and keeps
    // its own page.
    let hoist = kind == ItemKind::Enum && e.is_anonymous();
    let mut name = if hoist { String::new() } else { e.get_name()? };
    // Inside a class template, clang spells constructors and destructors
    // with the template arguments (`Stream<S, C>`, `~Stream<S, C>`);
    // strip to the plain class name.
    if matches!(kind, ItemKind::Constructor | ItemKind::Destructor)
        && let Some(pos) = name.find('<')
    {
        name.truncate(pos);
    }
    let qualified = if hoist {
        prefix.to_owned()
    } else if prefix.is_empty() {
        name.clone()
    } else {
        format!("{prefix}::{name}")
    };

    let mut item = Item::new(ctx.lang, kind, &name, &qualified);
    item.source = source_span(e, ctx);
    item.access = e.get_accessibility().map(|a| match a {
        Accessibility::Public => Access::Public,
        Accessibility::Protected => Access::Protected,
        Accessibility::Private => Access::Private,
    });

    // clang refuses to associate a doc comment when the spelled text
    // between it and the declaration name contains `;{}#@` — even inside
    // a macro argument's string literal, so `DEPRECATED("Use to_{a,b}")
    // int f();` silently loses its docs. Recover those ourselves.
    let comment = e.get_comment().or_else(|| recover_comment(e, floor, ctx));
    if let Some(raw) = comment {
        let docs = crate::comments::parse(&raw);
        if docs != DocBlock::default() {
            if item.since.is_none() {
                item.since = docs.since.clone();
            }
            if item.deprecated.is_none() {
                item.deprecated = docs.deprecated.clone();
            }
            item.docs = Some(docs);
        }
    }

    // An override without docs of its own inherits the overridden base
    // declaration's (Doxygen INHERIT_DOCS): the contract is the base's,
    // and restating it in every implementation is churn. `docs_from`
    // records the provenance — the base's parameter names may
    // legitimately differ from this signature's.
    if item.docs.is_none()
        && let Some((docs, from)) = inherited_docs(e)
    {
        item.docs = Some(docs);
        item.docs_from = Some(from);
    }

    apply_attribute_rules(e, &mut item, floor, ctx);
    if item.deprecated.is_none() && e.get_availability() == clang::Availability::Deprecated {
        item.deprecated = Some(String::new());
    }

    match kind {
        ItemKind::Namespace => {
            item.children = convert_scope_children(e, &qualified, ctx);
        }
        ItemKind::Class | ItemKind::Struct => {
            item.signature = template_signature(e);
            let mut floors = FloorTracker::new(Some(e));
            for child in e.get_children() {
                let child_floor = floors.floor_for(&child);
                floors.update(&child);
                if child.get_kind() == EntityKind::BaseSpecifier {
                    if (child.get_accessibility() == Some(Accessibility::Public)
                        || kind == ItemKind::Struct)
                        && let Some(t) = child.get_type()
                    {
                        item.bases.push(t.get_display_name());
                    }
                    continue;
                }
                if child.get_accessibility() == Some(Accessibility::Private) {
                    continue;
                }
                if let Some(c) = convert(&child, &qualified, child_floor, ctx) {
                    if c.kind == ItemKind::Enum && c.name.is_empty() {
                        item.children.extend(c.children);
                    } else {
                        item.children.push(c);
                    }
                }
            }
        }
        ItemKind::Enum => {
            item.ty = e.get_enum_underlying_type().map(|t| t.get_display_name());
            let mut floors = FloorTracker::new(Some(e));
            for child in e.get_children() {
                let child_floor = floors.floor_for(&child);
                floors.update(&child);
                if child.get_kind() == EntityKind::EnumConstantDecl
                    && let Some(mut c) = convert(&child, &qualified, child_floor, ctx)
                {
                    // Prefer the header's spelled initializer: a
                    // dependent one (`BLOCK_SIZE = BS` in a template)
                    // "computes" to a meaningless 0, and `Legacy = Off`
                    // reads better than its numeric value. Bare
                    // auto-increment enumerators get the computed value.
                    c.value = spelled_initializer(&child)
                        .or_else(|| child.get_enum_constant_value().map(|(s, _u)| s.to_string()));
                    item.children.push(c);
                }
            }
            if hoist {
                // The wrapper's comment and attributes describe the
                // constants it declares.
                for c in &mut item.children {
                    if c.docs.is_none() {
                        c.docs = item.docs.clone();
                    }
                    if c.since.is_none() {
                        c.since = item.since.clone();
                    }
                    if c.deprecated.is_none() {
                        c.deprecated = item.deprecated.clone();
                    }
                    if c.access.is_none() {
                        c.access = item.access;
                    }
                }
            }
        }
        ItemKind::Function
        | ItemKind::Method
        | ItemKind::Constructor
        | ItemKind::Destructor
        | ItemKind::ConversionFunction => {
            item.signature = Some(function_signature(e));
        }
        ItemKind::TypeAlias => {
            item.ty = e
                .get_typedef_underlying_type()
                .map(|t| t.get_display_name());
        }
        ItemKind::Variable | ItemKind::Field => {
            item.ty = e.get_type().map(|t| t.get_display_name());
        }
        ItemKind::Enumerator => {}
        _ => {}
    }

    Some(item)
}

/// Docs from the closest documented declaration in the override chain,
/// with its `Class::method` name for provenance.
fn inherited_docs(e: &Entity) -> Option<(DocBlock, String)> {
    for base in e.get_overridden_methods()? {
        if let Some(raw) = base.get_comment() {
            let docs = crate::comments::parse(&raw);
            if docs != DocBlock::default() {
                let from = match base.get_semantic_parent().and_then(|p| p.get_name()) {
                    Some(parent) => format!("{parent}::{}", base.get_name().unwrap_or_default()),
                    None => base.get_name().unwrap_or_default(),
                };
                return Some((docs, from));
            }
        }
        if let Some(found) = inherited_docs(&base) {
            return Some(found);
        }
    }
    None
}

/// Is this function-like declaration `= delete`d? The clang crate does
/// not bind clang_CXXMethod_isDeleted, so read the spelled declaration
/// tail: everything after the last `=` (which handles `operator=`) must
/// be the keyword. `= default` and pure-virtual `= 0` stay documented.
fn is_deleted_function(e: &Entity, ctx: &mut Ctx) -> bool {
    let Some(range) = e.get_range() else {
        return false;
    };
    let start = range.get_start().get_file_location();
    let end = range.get_end().get_file_location();
    let Some(file) = start.file else {
        return false;
    };
    let Some(w) = ctx.window(&file.get_path(), start.offset, end.offset) else {
        return false;
    };
    let Some(eq) = w.rfind('=') else {
        return false;
    };
    let tail: String = w[eq + 1..].chars().filter(|c| !c.is_whitespace()).collect();
    // Plain `= delete`, or C++26 `= delete("reason")`.
    tail == "delete" || tail.starts_with("delete(")
}

/// Fallback for clang's comment association: scan the floor-bounded
/// window before the declaration's name (the same window the attribute
/// rules use) for the last leading doc comment. Trailing-form markers
/// (`/**<`, `///<`) document the *previous* declaration and are never
/// picked up; adjacent `///` lines merge into one comment.
fn recover_comment(e: &Entity, floor: u32, ctx: &mut Ctx) -> Option<String> {
    let range = e.get_range()?;
    let name_loc = e.get_location()?.get_file_location();
    let file = name_loc.file?;
    let ext_start = range.get_start().get_file_location().offset;
    let scan_start = ext_start.min(floor);
    if name_loc.offset <= scan_start {
        return None;
    }
    // No comment opener in the raw window means nothing to recover —
    // skip the tokenization, which is far too expensive to run for
    // every undocumented declaration.
    if let Some(w) = ctx.window(&file.get_path(), scan_start, name_loc.offset)
        && !w.contains("/*")
        && !w.contains("//")
    {
        return None;
    }
    let scan_range = clang::source::SourceRange::new(
        file.get_offset_location(scan_start),
        file.get_offset_location(name_loc.offset),
    );
    let mut run: Vec<String> = Vec::new();
    let mut run_end_line = 0u32;
    for tok in scan_range.tokenize() {
        if tok.get_kind() != clang::token::TokenKind::Comment {
            continue;
        }
        let s = tok.get_spelling();
        let doc_start = ["/**", "/*!", "///", "//!"]
            .iter()
            .any(|p| s.starts_with(p));
        let trailing_form = ["/**<", "/*!<", "///<", "//!<"]
            .iter()
            .any(|p| s.starts_with(p));
        if !doc_start || trailing_form {
            run.clear();
            continue;
        }
        let line = tok.get_range().get_start().get_file_location().line;
        if !(s.starts_with("//") && !run.is_empty() && line == run_end_line + 1) {
            run.clear();
        }
        run.push(s);
        run_end_line = tok.get_range().get_end().get_file_location().line;
    }
    (!run.is_empty()).then(|| run.join("\n"))
}

/// The spelled initializer of an enumerator (`BLOCK_SIZE = BS` -> "BS").
fn spelled_initializer(e: &Entity) -> Option<String> {
    let tokens = e.get_range()?.tokenize();
    let eq = tokens.iter().position(|t| t.get_spelling() == "=")?;
    let mut out = String::new();
    for t in &tokens[eq + 1..] {
        let s = t.get_spelling();
        let adjacent_words = matches!(
            (out.chars().last(), s.chars().next()),
            (Some(a), Some(b))
                if (a.is_alphanumeric() || a == '_') && (b.is_alphanumeric() || b == '_')
        );
        if adjacent_words {
            out.push(' ');
        }
        out.push_str(&s);
    }
    (!out.is_empty()).then_some(out)
}

fn source_span(e: &Entity, ctx: &mut Ctx) -> Option<SourceSpan> {
    let range = e.get_range()?;
    let start = range.get_start().get_file_location();
    let end = range.get_end().get_file_location();
    let file = start.file?.get_path();
    Some(SourceSpan {
        file: ctx.canonical_display(file),
        line_start: start.line,
        line_end: end.line,
    })
}

fn function_signature(e: &Entity) -> Signature {
    let mut sig = Signature::default();
    // For a constructor/destructor template the cursor is a
    // FunctionTemplate; the templated declaration's kind is what rules
    // out the (spurious `void`) return type.
    let decl_kind = e.get_template_kind().unwrap_or_else(|| e.get_kind());
    if !matches!(decl_kind, EntityKind::Constructor | EntityKind::Destructor) {
        sig.return_type = e.get_result_type().map(|t| t.get_display_name());
    }
    for child in e.get_children() {
        match child.get_kind() {
            EntityKind::ParmDecl => {
                sig.params.push(Param {
                    name: child.get_name(),
                    ty: child
                        .get_type()
                        .map(|t| t.get_display_name())
                        .unwrap_or_default(),
                    default: param_default(&child),
                });
            }
            EntityKind::OverrideAttr => sig.is_override = true,
            EntityKind::FinalAttr => sig.is_final = true,
            _ => {}
        }
    }
    sig.is_const = e.is_const_method();
    sig.is_virtual = e.is_virtual_method();
    sig.is_pure_virtual = e.is_pure_virtual_method();
    sig.is_static = e.is_static_method();
    sig.is_noexcept = matches!(
        e.get_exception_specification(),
        Some(ExceptionSpecification::BasicNoexcept)
    );
    if let Some(t) = template_signature(e) {
        sig.template_params = t.template_params;
    }
    sig
}

/// Extract `template<...>` parameters (for ClassTemplate/FunctionTemplate).
fn template_signature(e: &Entity) -> Option<Signature> {
    let mut params: Vec<String> = Vec::new();
    for child in e.get_children() {
        let p = match child.get_kind() {
            EntityKind::TemplateTypeParameter => {
                format!("typename {}", child.get_name().unwrap_or_default())
            }
            EntityKind::NonTypeTemplateParameter => format!(
                "{} {}",
                child
                    .get_type()
                    .map(|t| t.get_display_name())
                    .unwrap_or_default(),
                child.get_name().unwrap_or_default()
            ),
            EntityKind::TemplateTemplateParameter => {
                format!("template<...> {}", child.get_name().unwrap_or_default())
            }
            _ => continue,
        };
        params.push(p.trim_end().to_owned());
    }
    if params.is_empty() {
        return None;
    }
    Some(Signature {
        template_params: Some(format!("template<{}>", params.join(", "))),
        ..Signature::default()
    })
}

/// Default argument, recovered from the parameter's spelled tokens
/// (everything after the top-level `=`).
fn param_default(param: &Entity) -> Option<String> {
    let tokens = param.get_range()?.tokenize();
    let mut depth = 0i32;
    for (i, tok) in tokens.iter().enumerate() {
        match tok.get_spelling().as_str() {
            "(" | "[" | "{" | "<" => depth += 1,
            ")" | "]" | "}" | ">" => depth -= 1,
            "=" if depth == 0 => {
                let text = join_tokens(tokens[i + 1..].iter().map(|t| t.get_spelling()));
                return (!text.is_empty()).then_some(text);
            }
            _ => {}
        }
    }
    None
}

/// Join token spellings with sensible spacing (no space around `::`, none
/// before `,`/`)`/`(`).
fn join_tokens(spellings: impl Iterator<Item = String>) -> String {
    let mut out = String::new();
    let mut prev_glue = true;
    for s in spellings {
        let glue_before = matches!(s.as_str(), "::" | "," | ")" | "]" | ">" | "(" | ";");
        if !out.is_empty() && !prev_glue && !glue_before {
            out.push(' ');
        }
        out.push_str(&s);
        prev_glue = matches!(s.as_str(), "::" | "(" | "[" | "<");
    }
    out
}

/// Scan the declaration's spelled tokens for configured attribute macros
/// and apply their effects (since/deprecated) to the item.
///
/// The scanned window runs from `floor` (end of the previous sibling — a
/// leading attribute macro is spelled *before* the declaration's AST
/// extent) to the entity's name. Stopping at the name keeps a class's
/// body from leaking its members' macros onto the class itself.
/// Enumerators are the exception: their deprecation macro follows the
/// name (`Legacy DEPRECATED("...") = Off`), so scan their whole extent.
fn apply_attribute_rules(e: &Entity, item: &mut Item, floor: u32, ctx: &mut Ctx) {
    if ctx.rules.is_empty() {
        return;
    }
    let Some(range) = e.get_range() else { return };
    let Some(name_loc) = e.get_location().map(|l| l.get_file_location()) else {
        return;
    };
    let Some(file) = name_loc.file else { return };

    let ext_start = range.get_start().get_file_location().offset;
    let scan_start = ext_start.min(floor);
    let scan_end = if e.get_kind() == EntityKind::EnumConstantDecl {
        range.get_end().get_file_location().offset
    } else {
        name_loc.offset
    };

    // Most declarations have no attribute macro in front of them: a
    // substring check on the raw source skips the per-declaration
    // libclang tokenization, which dominates extraction time otherwise.
    let rules = ctx.rules;
    if let Some(w) = ctx.window(&file.get_path(), scan_start, scan_end)
        && !rules.iter().any(|r| w.contains(&r.macro_name))
    {
        return;
    }

    let scan_range = clang::source::SourceRange::new(
        file.get_offset_location(scan_start),
        file.get_offset_location(scan_end),
    );

    // Collect spelled tokens, skipping comments and preprocessor
    // directive lines (a `#define TEST_API(...)` in the window must not
    // read as an invocation). Multi-line directives (`\` continuations)
    // are not handled; they don't occur between declarations in practice.
    let mut spellings: Vec<String> = Vec::new();
    let mut directive_line: Option<u32> = None;
    for tok in scan_range.tokenize() {
        let line = tok.get_range().get_start().get_file_location().line;
        if directive_line == Some(line) {
            continue;
        }
        directive_line = None;
        if tok.get_kind() == clang::token::TokenKind::Comment {
            continue;
        }
        let spelling = tok.get_spelling();
        if spelling == "#" {
            directive_line = Some(line);
            continue;
        }
        spellings.push(spelling);
    }

    for rule in ctx.rules {
        let Some(pos) = spellings.iter().position(|s| *s == rule.macro_name) else {
            continue;
        };
        let args = parse_macro_args(&spellings[pos + 1..]);
        let subst = |template: &str| -> String {
            let mut out = template.to_owned();
            for (name, value) in rule.arg_names.iter().zip(args.iter()) {
                out = out.replace(&format!("{{{name}}}"), value);
            }
            out
        };
        if let Some(t) = &rule.since {
            item.since = Some(subst(t));
        }
        if let Some(t) = &rule.deprecated {
            item.deprecated = Some(subst(t));
        }
    }
}

/// Given tokens starting after a macro name, parse `( a, b, ... )` into
/// top-level comma-separated arguments. String literals lose their quotes.
fn parse_macro_args(tokens: &[String]) -> Vec<String> {
    if tokens.first().map(String::as_str) != Some("(") {
        return Vec::new();
    }
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut depth = 1i32;
    let mut current: Vec<String> = Vec::new();
    for tok in &tokens[1..] {
        match tok.as_str() {
            "(" => {
                depth += 1;
                current.push(tok.clone());
            }
            ")" => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                current.push(tok.clone());
            }
            "," if depth == 1 => {
                groups.push(std::mem::take(&mut current));
            }
            _ => current.push(tok.clone()),
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
        .into_iter()
        .map(|toks| {
            let joined = join_tokens(toks.into_iter());
            joined
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .map(str::to_owned)
                .unwrap_or(joined)
        })
        .collect()
}

/// C++ allows reopening namespaces; the AST then has one Namespace cursor
/// per reopening. Merge same-name namespace items, preserving order, then
/// recurse: merging two `Botan` reopenings can itself bring two `PK_Ops`
/// children together.
fn merge_namespaces(items: &mut Vec<Item>) {
    let mut merged: Vec<Item> = Vec::new();
    let mut index_by_name: HashMap<String, usize> = HashMap::new();
    for item in items.drain(..) {
        if item.kind == ItemKind::Namespace {
            if let Some(&i) = index_by_name.get(&item.name) {
                let target = &mut merged[i];
                target.children.extend(item.children);
                if target.docs.is_none() {
                    target.docs = item.docs;
                }
                continue;
            }
            index_by_name.insert(item.name.clone(), merged.len());
        }
        merged.push(item);
    }
    for item in &mut merged {
        if item.kind == ItemKind::Namespace {
            merge_namespaces(&mut item.children);
        }
    }
    *items = merged;
}

/// Convenience for tests and callers that just want the items.
pub fn extract_header(header: &Path, attributes: Vec<AttributeRule>) -> Result<ExtractOutput> {
    extract(&CppConfig {
        std: "c++20".into(),
        c_mode: false,
        language: None,
        include_dirs: vec![],
        defines: vec![],
        extra_args: vec![],
        headers: vec![header.to_path_buf()],
        external_headers: Vec::new(),
        attributes,
    })
}
