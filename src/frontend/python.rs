//! The Python frontend: fully static extraction via ruff's parser.
//! The documented package is never imported, so doc builds need neither
//! a Python environment nor a built native library.
//!
//! Signature fragments (annotations, defaults, base classes) are sliced
//! verbatim from the source via AST ranges — what the author wrote is
//! what renders.

use std::path::{Path, PathBuf};

use crate::ir::{DocBlock, Item, ItemKind, Language, Module, Param, Signature, SourceSpan};
use anyhow::{Context, Result};
use ruff_python_ast::{
    Decorator, Expr, Parameter, ParameterWithDefault, Stmt, StmtClassDef, StmtFunctionDef,
};
use ruff_text_size::Ranged;

#[derive(Debug, Clone)]
pub struct PyConfig {
    /// Python module files to document (module name = file stem).
    pub modules: Vec<PathBuf>,
}

pub struct ExtractOutput {
    pub items: Vec<Item>,
    pub modules: Vec<Module>,
    pub warnings: Vec<String>,
}

pub fn extract(cfg: &PyConfig) -> Result<ExtractOutput> {
    let mut out = ExtractOutput {
        items: Vec::new(),
        modules: Vec::new(),
        warnings: Vec::new(),
    };
    for path in &cfg.modules {
        extract_module(path, &mut out).with_context(|| format!("extracting {}", path.display()))?;
    }
    Ok(out)
}

struct Ctx<'a> {
    source: &'a str,
    file: String,
    module_id: String,
    line_starts: Vec<usize>,
}

impl Ctx<'_> {
    fn slice(&self, node: &impl Ranged) -> String {
        self.source[node.range()].to_owned()
    }

    fn line_of(&self, offset: usize) -> u32 {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i as u32 + 1,
            Err(i) => i as u32,
        }
    }

    fn span(&self, node: &impl Ranged) -> SourceSpan {
        SourceSpan {
            file: self.file.clone(),
            line_start: self.line_of(node.range().start().to_usize()),
            line_end: self.line_of(node.range().end().to_usize()),
        }
    }

    /// 1-based line text, without the trailing newline.
    fn line(&self, ln: usize) -> &str {
        let start = self.line_starts[ln - 1];
        let end = self
            .line_starts
            .get(ln)
            .copied()
            .unwrap_or(self.source.len());
        self.source[start..end].trim_end_matches('\n')
    }

    /// The Sphinx `#:` doc comment for an assignment: trailing on the
    /// same line (`X = 1  #: doc`), or on the line(s) directly above.
    /// Searching after the node's end keeps `#:` inside the value (say,
    /// in a string literal) from being mistaken for a doc.
    fn hash_colon_doc(&self, node: &impl Ranged) -> Option<String> {
        let end = node.range().end().to_usize();
        let line_end = self.source[end..]
            .find('\n')
            .map_or(self.source.len(), |p| end + p);
        if let Some(p) = self.source[end..line_end].find("#:") {
            let text = self.source[end + p + 2..line_end].trim();
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
        let mut docs: Vec<&str> = Vec::new();
        let mut ln = (self.line_of(node.range().start().to_usize()) as usize).checked_sub(1)?;
        while ln >= 1 {
            match self.line(ln).trim_start().strip_prefix("#:") {
                Some(rest) => docs.push(rest.trim()),
                None => break,
            }
            ln -= 1;
        }
        docs.reverse();
        (!docs.is_empty()).then(|| docs.join("\n"))
    }
}

fn extract_module(path: &Path, out: &mut ExtractOutput) -> Result<()> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("module not found: {}", path.display()))?;
    let source = std::fs::read_to_string(&canonical)?;
    let module_id = canonical
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .context("module file has no stem")?;

    if out.modules.iter().any(|m| m.id == module_id) {
        anyhow::bail!(
            "duplicate python module name `{module_id}`: modules are identified by file \
             stem, so multi-file packages need distinct stems \
             (package-qualified module identity is not yet supported)"
        );
    }

    let parsed = ruff_python_parser::parse_module(&source)
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    let mut line_starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let ctx = Ctx {
        source: &source,
        file: canonical.display().to_string(),
        module_id: module_id.clone(),
        line_starts,
    };

    let body = &parsed.syntax().body;
    out.modules.push(Module {
        id: module_id.clone(),
        name: module_id.clone(),
        brief: docstring_of(body).and_then(|d| {
            let parsed = parse_docstring(&d);
            parsed.summary
        }),
        parent: None,
        headers: vec![ctx.file.clone()],
    });

    // `__all__` is the author's explicit export list; when present it
    // decides module-level inclusion instead of the underscore
    // convention (Sphinx automodule semantics).
    let exported = explicit_exports(body);
    let included = |name: &str, default: bool| match &exported {
        Some(set) => set.contains(name),
        None => default,
    };

    for stmt in body {
        match stmt {
            Stmt::FunctionDef(f) if included(f.name.as_str(), is_public(f.name.as_str())) => {
                out.items.push(convert_function(f, &ctx, None));
            }
            Stmt::ClassDef(c) if included(c.name.as_str(), is_public(c.name.as_str())) => {
                out.items.push(convert_class(c, &ctx));
            }
            Stmt::Assign(a) => {
                if let [Expr::Name(n)] = a.targets.as_slice()
                    && n.id.as_str() != "__all__"
                {
                    let doc = ctx.hash_colon_doc(stmt);
                    let default = is_public_constant(n.id.as_str())
                        || (is_public(n.id.as_str()) && doc.is_some());
                    if included(n.id.as_str(), default) {
                        let mut item = leaf_item(ItemKind::Variable, n.id.as_str(), &ctx, stmt);
                        item.value = Some(truncate(&ctx.slice(&*a.value), 80));
                        item.docs = doc.as_deref().map(parse_docstring);
                        out.items.push(item);
                    }
                }
            }
            Stmt::AnnAssign(a) => {
                if let Expr::Name(n) = &*a.target {
                    let doc = ctx.hash_colon_doc(stmt);
                    let default = is_public_constant(n.id.as_str())
                        || (is_public(n.id.as_str()) && doc.is_some());
                    if included(n.id.as_str(), default) {
                        let mut item = leaf_item(ItemKind::Variable, n.id.as_str(), &ctx, stmt);
                        item.ty = Some(ctx.slice(&*a.annotation));
                        item.value = a.value.as_deref().map(|v| truncate(&ctx.slice(v), 80));
                        item.docs = doc.as_deref().map(parse_docstring);
                        out.items.push(item);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The string elements of a module-level `__all__ = [...]`, if any.
fn explicit_exports(body: &[Stmt]) -> Option<std::collections::HashSet<String>> {
    for stmt in body {
        let Stmt::Assign(a) = stmt else { continue };
        let [Expr::Name(n)] = a.targets.as_slice() else {
            continue;
        };
        if n.id.as_str() != "__all__" {
            continue;
        }
        let elts = match &*a.value {
            Expr::List(l) => &l.elts,
            Expr::Tuple(t) => &t.elts,
            _ => return None,
        };
        return Some(
            elts.iter()
                .filter_map(|e| match e {
                    Expr::StringLiteral(s) => Some(s.value.to_str().to_owned()),
                    _ => None,
                })
                .collect(),
        );
    }
    None
}

/// Public API: underscore convention (`__all__` support can come later —
/// Botan's binding doesn't define one).
fn is_public(name: &str) -> bool {
    !name.starts_with('_')
}

/// Module-level assignments documented when they look like constants;
/// any public name qualifies if it carries a `#:` doc comment (the
/// Sphinx autodata convention — catches type aliases like `MPILike`).
fn is_public_constant(name: &str) -> bool {
    is_public(name)
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
}

/// Dunder methods worth documenting when they appear on a public class.
const DUNDER_ALLOWLIST: &[&str] = &[
    "__init__",
    "__enter__",
    "__exit__",
    "__len__",
    "__getitem__",
    "__setitem__",
    "__iter__",
    "__next__",
    "__contains__",
];

fn method_is_public(name: &str) -> bool {
    is_public(name) || DUNDER_ALLOWLIST.contains(&name)
}

fn leaf_item(kind: ItemKind, name: &str, ctx: &Ctx, node: &impl Ranged) -> Item {
    let mut item = Item::new(Language::Python, kind, name, name);
    item.id = format!("py:{}.{}", ctx.module_id, name);
    item.module = Some(ctx.module_id.clone());
    item.source = Some(ctx.span(node));
    item
}

fn convert_class(c: &StmtClassDef, ctx: &Ctx) -> Item {
    let name = c.name.to_string();
    let mut item = Item::new(Language::Python, ItemKind::Class, &name, &name);
    item.id = format!("py:{}.{}", ctx.module_id, name);
    item.module = Some(ctx.module_id.clone());
    item.source = Some(ctx.span(c));
    if let Some(args) = &c.arguments {
        item.bases = args.args.iter().map(|b| ctx.slice(b)).collect();
    }
    if let Some(d) = docstring_of(&c.body) {
        item.docs = Some(parse_docstring(&d));
    }
    for stmt in &c.body {
        match stmt {
            Stmt::FunctionDef(f) if method_is_public(f.name.as_str()) => {
                item.children.push(convert_function(f, ctx, Some(&name)));
            }
            Stmt::AnnAssign(a) => {
                if let Expr::Name(n) = &*a.target
                    && is_public(n.id.as_str())
                {
                    let mut attr = leaf_item(ItemKind::Field, n.id.as_str(), ctx, stmt);
                    attr.qualified_name = format!("{name}.{}", n.id);
                    attr.id = format!("py:{}.{}", ctx.module_id, attr.qualified_name);
                    attr.ty = Some(ctx.slice(&*a.annotation));
                    attr.docs = ctx.hash_colon_doc(stmt).as_deref().map(parse_docstring);
                    item.children.push(attr);
                }
            }
            // Unannotated class attributes: enum-style value tables
            // (`unspecified = 0`) and plain class constants.
            Stmt::Assign(a) => {
                if let [Expr::Name(n)] = a.targets.as_slice()
                    && is_public(n.id.as_str())
                {
                    let mut attr = leaf_item(ItemKind::Field, n.id.as_str(), ctx, stmt);
                    attr.qualified_name = format!("{name}.{}", n.id);
                    attr.id = format!("py:{}.{}", ctx.module_id, attr.qualified_name);
                    attr.value = Some(truncate(&ctx.slice(&*a.value), 80));
                    attr.docs = ctx.hash_colon_doc(stmt).as_deref().map(parse_docstring);
                    item.children.push(attr);
                }
            }
            _ => {}
        }
    }
    item
}

fn convert_function(f: &StmtFunctionDef, ctx: &Ctx, class: Option<&str>) -> Item {
    let name = f.name.to_string();
    let is_property = has_decorator(&f.decorator_list, "property");
    let is_static = has_decorator(&f.decorator_list, "staticmethod");
    let is_classmethod = has_decorator(&f.decorator_list, "classmethod");

    let kind = match (class, is_property, name.as_str()) {
        (Some(_), true, _) => ItemKind::Field,
        (Some(_), _, "__init__") => ItemKind::Constructor,
        (Some(_), _, _) => ItemKind::Method,
        (None, _, _) => ItemKind::Function,
    };

    let qualified = match class {
        Some(cls) => format!("{cls}.{name}"),
        None => name.clone(),
    };
    let mut item = Item::new(Language::Python, kind, &name, &qualified);
    item.id = format!("py:{}.{}", ctx.module_id, qualified);
    item.module = Some(ctx.module_id.clone());
    item.source = Some(ctx.span(f));

    if kind == ItemKind::Field {
        // Property: document as an attribute typed by the return annotation.
        item.ty = f.returns.as_deref().map(|r| ctx.slice(r));
    } else {
        let mut sig = Signature {
            return_type: f.returns.as_deref().map(|r| ctx.slice(r)),
            is_static,
            ..Signature::default()
        };
        let skip_self = class.is_some() && !is_static;
        sig.params = convert_params(f, ctx, skip_self, is_classmethod);
        item.signature = Some(sig);
    }

    if let Some(d) = docstring_of(&f.body) {
        let docs = parse_docstring(&d);
        item.deprecated = docs.deprecated.clone();
        item.docs = Some(docs);
    }
    item
}

fn convert_params(f: &StmtFunctionDef, ctx: &Ctx, skip_self: bool, skip_cls: bool) -> Vec<Param> {
    let mut params = Vec::new();
    let mut first = true;
    let p = &f.parameters;

    let push_pd = |pd: &ParameterWithDefault, params: &mut Vec<Param>, first: &mut bool| {
        let name = pd.parameter.name.to_string();
        if *first && (skip_self && name == "self" || skip_cls && name == "cls") {
            *first = false;
            return;
        }
        *first = false;
        params.push(Param {
            name: Some(name),
            ty: pd
                .parameter
                .annotation
                .as_deref()
                .map(|a| ctx.slice(a))
                .unwrap_or_default(),
            default: pd.default.as_deref().map(|d| ctx.slice(d)),
        });
    };

    for pd in &p.posonlyargs {
        push_pd(pd, &mut params, &mut first);
    }
    if !p.posonlyargs.is_empty() {
        params.push(marker("/"));
    }
    for pd in &p.args {
        push_pd(pd, &mut params, &mut first);
    }
    match (&p.vararg, p.kwonlyargs.is_empty()) {
        (Some(v), _) => params.push(star_param(v, ctx, "*")),
        (None, false) => params.push(marker("*")),
        _ => {}
    }
    for pd in &p.kwonlyargs {
        push_pd(pd, &mut params, &mut first);
    }
    if let Some(k) = &p.kwarg {
        params.push(star_param(k, ctx, "**"));
    }
    params
}

fn marker(name: &str) -> Param {
    Param {
        name: Some(name.to_owned()),
        ty: String::new(),
        default: None,
    }
}

fn star_param(p: &Parameter, ctx: &Ctx, stars: &str) -> Param {
    Param {
        name: Some(format!("{stars}{}", p.name)),
        ty: p
            .annotation
            .as_deref()
            .map(|a| ctx.slice(a))
            .unwrap_or_default(),
        default: None,
    }
}

fn has_decorator(decorators: &[Decorator], name: &str) -> bool {
    decorators.iter().any(|d| match &d.expression {
        Expr::Name(n) => n.id == name,
        _ => false,
    })
}

fn docstring_of(body: &[Stmt]) -> Option<String> {
    let Some(Stmt::Expr(e)) = body.first() else {
        return None;
    };
    let Expr::StringLiteral(s) = &*e.value else {
        return None;
    };
    Some(s.value.to_str().to_owned())
}

/// Dedent (docstring continuation lines carry the def's indentation,
/// which Markdown would read as a code block) and parse.
fn parse_docstring(raw: &str) -> DocBlock {
    let mut lines: Vec<&str> = raw.lines().collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    for l in lines.iter_mut().skip(1) {
        *l = if l.len() >= indent {
            &l[indent..]
        } else {
            l.trim_start()
        };
    }
    crate::comments::parse(lines.join("\n").trim())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let cut = (0..=max)
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0);
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_src(src: &str) -> ExtractOutput {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "udg-py-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mod_under_test.py");
        std::fs::write(&path, src).unwrap();
        let out = extract(&PyConfig {
            modules: vec![path],
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn hash_colon_docs_admit_and_document_aliases() {
        let out = extract_src(
            "UPPER = 1\n\
             MPILike = Union[str, \"MPI\", None]  #: Alias for MPI-able values.\n\
             #: Callback prototype,\n\
             #: two lines of it.\n\
             Handler = Callable[[int], None]\n\
             undocumented_alias = int\n\
             NOT_A_DOC = \"#: inside a string\"\n",
        );
        let find = |n: &str| out.items.iter().find(|i| i.name == n);

        // Bare constants still extract, docs or not.
        assert!(find("UPPER").is_some());
        // A `#:` doc admits a non-ALL-CAPS alias, trailing form...
        let mpi = find("MPILike").expect("MPILike extracted");
        assert_eq!(
            mpi.docs.as_ref().and_then(|d| d.summary.as_deref()),
            Some("Alias for MPI-able values.")
        );
        // ...and preceding form: lines join into one summary paragraph.
        let handler = find("Handler").expect("Handler extracted");
        assert_eq!(
            handler.docs.as_ref().and_then(|d| d.summary.as_deref()),
            Some("Callback prototype, two lines of it.")
        );
        // Without one, a lowercase module assignment stays out.
        assert!(find("undocumented_alias").is_none());
        // `#:` inside the value is not a doc comment.
        assert!(find("NOT_A_DOC").unwrap().docs.is_none());
    }

    #[test]
    fn class_assignments_all_and_duplicate_modules() {
        // Note: no `\`-continuations — they would strip the Python
        // indentation the parser needs.
        let out = extract_src(concat!(
            "__all__ = [\"Reason\", \"listed\"]\n",
            "def listed():\n    pass\n",
            "def unlisted():\n    pass\n",
            "class Reason:\n",
            "    unspecified = 0\n",
            "    key_compromise = 1  #: the key leaked\n",
            "    _hidden = 2\n",
            "class NotExported:\n    pass\n",
        ));
        // __all__ decides module-level inclusion.
        let names: Vec<&str> = out.items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"listed") && names.contains(&"Reason"),
            "{names:?}"
        );
        assert!(!names.contains(&"unlisted") && !names.contains(&"NotExported"));
        assert!(!names.contains(&"__all__"));

        // Enum-style class attributes extract with values and #: docs.
        let reason = out.items.iter().find(|i| i.name == "Reason").unwrap();
        let attr = |n: &str| reason.children.iter().find(|c| c.name == n);
        assert_eq!(attr("unspecified").unwrap().value.as_deref(), Some("0"));
        let kc = attr("key_compromise").unwrap();
        assert_eq!(kc.value.as_deref(), Some("1"));
        assert_eq!(
            kc.docs.as_ref().and_then(|d| d.summary.as_deref()),
            Some("the key leaked")
        );
        assert!(attr("_hidden").is_none());

        // Two files with the same stem cannot silently merge.
        let dir = std::env::temp_dir().join(format!("udg-py-dup-{}", std::process::id()));
        for sub in ["a", "b"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::write(dir.join(sub).join("client.py"), "x = 1\n").unwrap();
        }
        let msg = match extract(&PyConfig {
            modules: vec![dir.join("a/client.py"), dir.join("b/client.py")],
        }) {
            Ok(_) => panic!("duplicate module stems must fail"),
            Err(e) => format!("{e:#}"),
        };
        assert!(msg.contains("duplicate python module"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
