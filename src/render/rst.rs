//! Native reStructuredText rendering (second pass): a block-structured
//! parser emitting HTML directly — no Markdown intermediate — covering
//! the rst *prose dialect*: sections, paragraphs, bullet/enumerated
//! lists, literal blocks, block quotes, directives, inline markup and
//! roles, and both grid and simple tables.
//!
//! Unknown directives keep their body and produce a warning.

use std::sync::LazyLock;

use std::collections::HashMap;

use regex::Regex;

use crate::render::html_escape;
use crate::render::md::Resolver;

/// A transclusion request (`.. udg:member::` or a Sphinx autodoc
/// directive): item name in, member HTML out (None when the name does
/// not resolve).
pub struct EmbedRequest<'a> {
    pub name: &'a str,
    /// Language the directive implies (autodoc is the Python domain);
    /// None lets the resolver use its cross-language preference order.
    pub lang: Option<crate::ir::Language>,
    /// `:members:` — None: just the item; Some but empty: all public
    /// documented members; Some(names): exactly the listed members.
    pub members: Option<Vec<String>>,
}

pub type EmbedFn<'r> = &'r dyn Fn(&EmbedRequest) -> Option<String>;

static DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)\.\.\s+([A-Za-z:_-]+)::\s*(.*)$").unwrap());
static SIMPLE_BORDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^=+(\s+=+)+\s*$").unwrap());
static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)[-*]\s+\S").unwrap());
static ENUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)(\d+[.)]|#\.)\s+\S").unwrap());

const SIG_DIRECTIVES: &[&str] = &[
    "cpp:function",
    "cpp:class",
    "cpp:struct",
    "cpp:enum",
    "cpp:enum-class",
    "cpp:enumerator",
    "cpp:type",
    "cpp:member",
    "cpp:var",
    "c:function",
    "c:type",
    "c:macro",
    "c:var",
    "c:enum",
    "c:struct",
    "py:function",
    "py:class",
    "py:method",
    "py:classmethod",
    "py:staticmethod",
    "py:attribute",
    "py:data",
    "py:exception",
    "envvar",
];
const DROP_DIRECTIVES: &[&str] = &["toctree", "index", "contents", "default-domain"];
/// Sphinx autodoc directives, rendered as transclusions from the
/// extracted IR — udg never imports the documented module.
const AUTODOC_DIRECTIVES: &[&str] = &[
    "autofunction",
    "automethod",
    "autoclass",
    "autoexception",
    "autodata",
    "autoattribute",
];
/// Role names (or domain-qualified role suffixes) that mark code-ish
/// mentions, rendered as (auto-linked) code spans.
const CODE_ROLES: &[&str] = &[
    "func",
    "function",
    "class",
    "meth",
    "method",
    "attr",
    "type",
    "member",
    "var",
    "macro",
    "envvar",
    "data",
    "mod",
    "exc",
    "obj",
    "enum",
    "enumerator",
];

pub struct RstOutput {
    pub html: String,
    pub title: Option<String>,
    pub warnings: Vec<RstWarning>,
    /// Document names from `.. toctree::` directives, in order.
    pub toctree: Vec<(usize, String)>,
    /// Hand-written function-like signature directives (`cpp:function`
    /// etc.): (1-based line, directive language, signature text).
    /// Consumed by the guide-drift lint in `udg check` — the language
    /// keeps a Python item from vacuously satisfying a C++ directive.
    pub sig_directives: Vec<(usize, crate::ir::Language, String)>,
}

pub struct RstWarning {
    /// 1-based source line.
    pub line: usize,
    pub message: String,
}

struct Ctx<'r> {
    resolve: Option<Resolver<'r>>,
    embed: Option<EmbedFn<'r>>,
    /// Language applied to bare `::` literal blocks (Sphinx
    /// highlight_language / `.. highlight::`).
    default_lang: Option<String>,
    file: Option<std::path::PathBuf>,
    source_root: Option<std::path::PathBuf>,
    sig_directives: Vec<(usize, crate::ir::Language, String)>,
    warnings: Vec<(usize, String)>,
    toctree: Vec<(usize, String)>,
    heading_levels: Vec<char>,
    title: Option<String>,
    /// Current Python module (`.. py:module::` / `.. currentmodule::`):
    /// qualifies bare names in autodoc directives.
    py_module: Option<String>,
    /// `:ref:` label registry: label -> (page url, section title).
    ref_targets: Option<HashMap<String, (String, Option<String>)>>,
    /// `:doc:` registry: document stem -> page title.
    doc_pages: Option<HashMap<String, String>>,
    /// Filesystem boundary for literalinclude (canonicalized).
    include_root: Option<std::path::PathBuf>,
    /// Source line of the block being rendered (for inline warnings).
    line: usize,
}

/// Rendering context beyond the source text itself.
#[derive(Default)]
pub struct RstOptions<'a> {
    pub resolve: Option<Resolver<'a>>,
    /// Renders `.. udg:member:: Name` transclusions: given an item
    /// name, returns the member HTML, or None if unresolvable.
    pub embed: Option<EmbedFn<'a>>,
    /// Sphinx highlight_language: applied to bare `::` literal blocks.
    pub default_lang: Option<&'a str>,
    /// Path of the file being rendered (for relative literalinclude).
    pub file: Option<&'a std::path::Path>,
    /// Doc source root: `/`-prefixed literalinclude paths resolve here.
    pub source_root: Option<&'a std::path::Path>,
    /// `:ref:` label registry collected across the guide.
    pub ref_targets: Option<&'a HashMap<String, (String, Option<String>)>>,
    /// `:doc:` registry: document stem -> title.
    pub doc_pages: Option<&'a HashMap<String, String>>,
    /// literalinclude may only read files under this root (defaults to
    /// `source_root` when unset).
    pub include_root: Option<&'a std::path::Path>,
}

pub fn render(src: &str, opts: &RstOptions) -> RstOutput {
    let mut ctx = Ctx {
        resolve: opts.resolve,
        embed: opts.embed,
        default_lang: opts.default_lang.map(str::to_owned),
        file: opts.file.map(std::path::Path::to_path_buf),
        source_root: opts.source_root.map(std::path::Path::to_path_buf),
        sig_directives: Vec::new(),
        warnings: Vec::new(),
        toctree: Vec::new(),
        heading_levels: Vec::new(),
        title: None,
        py_module: None,
        ref_targets: opts.ref_targets.cloned(),
        doc_pages: opts.doc_pages.cloned(),
        include_root: opts
            .include_root
            .or(opts.source_root)
            .and_then(|p| p.canonicalize().ok()),
        line: 0,
    };
    let lines: Vec<&str> = src.lines().collect();
    let html = blocks(&lines, 0, &mut ctx);
    let mut w = ctx.warnings;
    w.sort();
    w.dedup();
    RstOutput {
        html,
        title: ctx.title,
        warnings: w
            .into_iter()
            .map(|(line, message)| RstWarning { line, message })
            .collect(),
        toctree: ctx.toctree,
        sig_directives: ctx.sig_directives,
    }
}

/// Directives whose argument is a checkable function-like signature.
const FN_SIG_DIRECTIVES: &[&str] = &[
    "cpp:function",
    "c:function",
    "py:function",
    "py:method",
    "py:classmethod",
    "py:staticmethod",
];

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `.. list-table::` — a table written as a bullet list of bullet
/// lists. `:header-rows:` renders leading rows as headers; layout
/// options (`:widths:`, `:align:`, ...) are presentational and
/// ignored.
fn list_table(title: &str, body: &[String], ctx: &mut Ctx) -> String {
    let mut header_rows = 0usize;
    let mut rows: Vec<Vec<String>> = Vec::new();
    for raw in body {
        let t = raw.trim_start();
        if t.is_empty() {
            continue;
        }
        if rows.is_empty() && t.starts_with(':') {
            if let Some(n) = t
                .strip_prefix(":header-rows:")
                .and_then(|v| v.trim().parse::<usize>().ok())
            {
                header_rows = n;
            }
            continue;
        }
        let indent = indent_of(raw);
        if let Some(rest) = t.strip_prefix('*') {
            rows.push(Vec::new());
            let rest = rest.trim_start();
            if let Some(cell) = rest.strip_prefix('-') {
                rows.last_mut()
                    .expect("just pushed")
                    .push(cell.trim().to_owned());
            }
        } else if indent <= 3
            && let Some(cell) = t.strip_prefix('-')
            && let Some(row) = rows.last_mut()
        {
            row.push(cell.trim().to_owned());
        } else if let Some(cell) = rows.last_mut().and_then(|r| r.last_mut()) {
            // Continuation line of the previous cell.
            cell.push(' ');
            cell.push_str(t);
        }
    }

    let mut out = String::from("<table class=\"rows doctable\">\n");
    if !title.trim().is_empty() {
        out.push_str(&format!(
            "<caption>{}</caption>\n",
            inline(title.trim(), ctx)
        ));
    }
    for (i, row) in rows.iter().enumerate() {
        let tag = if i < header_rows { "th" } else { "td" };
        out.push_str("<tr>");
        for cell in row {
            out.push_str(&format!("<{tag}>{}</{tag}>", inline(cell, ctx)));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
    out
}

/// Leading `:option:` lines of an autodoc directive body. Returns the
/// `:members:` request (None absent, empty = all, else the listed
/// names) and the remaining narrative body lines.
fn autodoc_options(
    directive: &str,
    body: &[String],
    base: usize,
    ctx: &mut Ctx,
) -> (Option<Vec<String>>, Vec<String>) {
    let mut members: Option<Vec<String>> = None;
    let mut i = 0;
    while i < body.len() {
        let t = body[i].trim();
        if t.is_empty() {
            if i == 0 {
                i += 1;
                continue;
            }
            break;
        }
        let Some((opt, val)) = t.strip_prefix(':').and_then(|r| r.split_once(':')) else {
            break;
        };
        match opt.trim() {
            "members" => {
                members = Some(
                    val.split([',', ' '])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                );
            }
            other => ctx.warnings.push((
                base + i + 1,
                format!("{directive}: option `:{other}:` ignored"),
            )),
        }
        i += 1;
    }
    (members, body[i..].to_vec())
}

pub(crate) fn adornment_char(line: &str) -> Option<char> {
    let t = line.trim_end();
    let first = t.chars().next()?;
    if "=-~^\"'#*+".contains(first) && t.len() >= 3 && t.chars().all(|c| c == first) {
        Some(first)
    } else {
        None
    }
}

fn is_grid_line(line: &str) -> bool {
    let t = line.trim();
    (t.starts_with('+') && t.len() > 2 && t.chars().all(|c| matches!(c, '+' | '-' | '=')))
        || t.starts_with('|')
}

fn dedent(lines: &[&str]) -> Vec<String> {
    let min = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.len() > min {
                l[min..].to_owned()
            } else {
                String::new()
            }
        })
        .collect()
}

/// `base`: 0-based line offset of this slice within the source file,
/// so warnings carry real line numbers through recursion.
fn blocks_owned(lines: &[String], base: usize, ctx: &mut Ctx) -> String {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    blocks(&refs, base, ctx)
}

fn blocks(lines: &[&str], base: usize, ctx: &mut Ctx) -> String {
    let n = lines.len();
    let mut out = String::new();
    let mut i = 0;

    let block_end = |start: usize, base_indent: usize| -> usize {
        let mut j = start;
        while j < n && (lines[j].trim().is_empty() || indent_of(lines[j]) > base_indent) {
            j += 1;
        }
        j
    };

    while i < n {
        let iter_start = i;
        ctx.line = base + i + 1;
        let line = lines[i];
        let stripped = line.trim();
        if stripped.is_empty() {
            i += 1;
            continue;
        }

        // Overline heading: adornment / text / same adornment. Must be
        // checked before the simple-table border (both may be all '=').
        if adornment_char(line).is_some()
            && i + 2 < n
            && lines[i + 2].trim() == stripped
            && !lines[i + 1].trim().is_empty()
        {
            i += 1; // skip the overline; the underline branch does the rest
            continue;
        }

        // Heading: text followed by an adornment underline.
        if indent_of(line) == 0
            && i + 1 < n
            && adornment_char(lines[i + 1]).is_some()
            && !SIMPLE_BORDER.is_match(lines[i + 1])
            && lines[i + 1].trim().len() + 2 >= stripped.len()
        {
            let ch = adornment_char(lines[i + 1]).unwrap();
            if !ctx.heading_levels.contains(&ch) {
                ctx.heading_levels.push(ch);
            }
            let level = ctx.heading_levels.iter().position(|c| *c == ch).unwrap() + 1;
            if level == 1 && ctx.title.is_none() {
                ctx.title = Some(stripped.to_owned());
            }
            out.push_str(&format!(
                "<h{0}>{1}</h{0}>\n",
                level.min(6),
                inline(stripped, ctx)
            ));
            i += 2;
            continue;
        }

        // Directives.
        if let Some(caps) = DIRECTIVE.captures(line) {
            let ind = caps.get(1).unwrap().as_str().len();
            let name = caps.get(2).unwrap().as_str().to_owned();
            let arg = caps.get(3).unwrap().as_str().to_owned();

            let mut j = i + 1;
            let mut args: Vec<String> = if arg.is_empty() {
                Vec::new()
            } else {
                vec![arg.clone()]
            };
            if SIG_DIRECTIVES.contains(&name.as_str()) || name == "udg:member" {
                let unbalanced = |s: &str| {
                    s.chars().fold(0i32, |d, c| match c {
                        '(' => d + 1,
                        ')' => d - 1,
                        _ => d,
                    }) > 0
                        || s.ends_with('\\')
                };
                while j < n && !lines[j].trim().is_empty() && indent_of(lines[j]) > ind {
                    let line = lines[j].trim();
                    match args.last_mut() {
                        // A wrapped declaration continues the previous
                        // line (open parens or trailing backslash);
                        // otherwise each line is its own declaration.
                        Some(last) if unbalanced(last) => {
                            while last.ends_with('\\') {
                                last.pop();
                            }
                            last.push(' ');
                            last.push_str(line);
                        }
                        _ => args.push(line.to_owned()),
                    }
                    j += 1;
                }
            }
            let end = block_end(j, ind);
            let body = dedent(&lines[j..end]);

            match name.as_str() {
                "code-block" | "sourcecode" | "code" => {
                    let lang = match arg.trim() {
                        "c++" => "cpp",
                        "" => "text",
                        other => other,
                    };
                    let mut b = body.clone();
                    while b.first().is_some_and(|l| l.trim().is_empty()) {
                        b.remove(0);
                    }
                    while b.last().is_some_and(|l| l.trim().is_empty()) {
                        b.pop();
                    }
                    let joined = b.join("\n");
                    let inner = crate::render::highlight::block(&format!("{joined}\n"), lang)
                        .unwrap_or_else(|| html_escape(&joined));
                    out.push_str(&format!(
                        "<pre><code class=\"language-{}\">{}</code></pre>\n",
                        html_escape(lang),
                        inner
                    ));
                }
                "udg:member" => {
                    // Transclusion: render the machine-generated member
                    // block for each named item — the single-source-of-
                    // truth alternative to hand-written signatures. The
                    // indented body (after a blank line) is narrative
                    // prose rendered beneath the embeds.
                    for name_arg in args.iter().filter(|n| !n.is_empty()) {
                        let req = EmbedRequest {
                            name: name_arg,
                            lang: None,
                            members: None,
                        };
                        match ctx.embed.and_then(|e| e(&req)) {
                            Some(html) => out.push_str(&html),
                            None => ctx.warnings.push((
                                base + i + 1,
                                format!("udg:member: cannot resolve `{name_arg}`"),
                            )),
                        }
                    }
                    out.push_str(&blocks_owned(&body, base + j, ctx));
                }
                "py:module" | "py:currentmodule" | "module" | "currentmodule" => {
                    let m = arg.trim();
                    if !m.is_empty() {
                        ctx.py_module = Some(m.to_owned());
                    }
                }
                _ if AUTODOC_DIRECTIVES.contains(&name.as_str()) => {
                    // Sphinx autodoc, spelled the way Sphinx projects
                    // already spell it — but resolved against the
                    // extracted IR instead of by importing the module.
                    // Leading `:option:` lines configure the embed; the
                    // rest of the body is narrative prose.
                    let (members, prose) = autodoc_options(&name, &body, base + j, ctx);
                    let bare = arg.trim().split('(').next().unwrap_or("").trim();
                    let mut candidates = vec![bare.to_owned()];
                    if let Some(m) = &ctx.py_module {
                        match bare.strip_prefix(&format!("{m}.")) {
                            Some(stripped) => candidates.push(stripped.to_owned()),
                            None => candidates.push(format!("{m}.{bare}")),
                        }
                    }
                    let html = ctx.embed.and_then(|e| {
                        candidates.iter().find_map(|c| {
                            e(&EmbedRequest {
                                name: c,
                                lang: Some(crate::ir::Language::Python),
                                members: members.clone(),
                            })
                        })
                    });
                    match html {
                        Some(h) if !bare.is_empty() => out.push_str(&h),
                        _ => ctx
                            .warnings
                            .push((base + i + 1, format!("{name}: cannot resolve `{bare}`"))),
                    }
                    out.push_str(&blocks_owned(
                        &prose,
                        base + j + (body.len() - prose.len()),
                        ctx,
                    ));
                }
                _ if SIG_DIRECTIVES.contains(&name.as_str()) => {
                    if FN_SIG_DIRECTIVES.contains(&name.as_str()) {
                        let lang = if name.starts_with("py:") {
                            crate::ir::Language::Python
                        } else if name.starts_with("c:") {
                            crate::ir::Language::C
                        } else {
                            crate::ir::Language::Cpp
                        };
                        for a in &args {
                            if !a.is_empty() {
                                ctx.sig_directives.push((base + i + 1, lang, a.clone()));
                            }
                        }
                    }
                    // Same visual language as reference pages: bordered
                    // member block, type names in the signature linked.
                    out.push_str("<div class=\"member\">\n");
                    for a in &args {
                        if !a.is_empty() {
                            out.push_str(&format!(
                                "<pre class=\"sig\">{}</pre>\n",
                                crate::render::linkify_sig(a, ctx.resolve)
                            ));
                        }
                    }
                    out.push_str(&blocks_owned(&body, base + j, ctx));
                    out.push_str("</div>\n");
                }
                "toctree" => {
                    for l in &body {
                        let t = l.trim();
                        if t.is_empty() || t.starts_with(':') {
                            continue;
                        }
                        let entry = match (t.rfind('<'), t.ends_with('>')) {
                            (Some(lt), true) => t[lt + 1..t.len() - 1].trim(),
                            _ => t,
                        };
                        ctx.toctree.push((base + i + 1, entry.to_owned()));
                    }
                }
                "note" | "tip" | "important" | "hint" => {
                    out.push_str(&admonition("note", &name, &args, &body, base + j, ctx));
                }
                "warning" | "caution" | "danger" | "attention" | "error" => {
                    out.push_str(&admonition("warning", &name, &args, &body, base + j, ctx));
                }
                "versionadded" | "versionchanged" | "deprecated" => {
                    let what = match name.as_str() {
                        "versionadded" => "Added in version",
                        "versionchanged" => "Changed in version",
                        _ => "Deprecated since version",
                    };
                    out.push_str(&format!(
                        "<p class=\"fieldline\"><span class=\"lbl\">{what} {}.</span></p>\n",
                        html_escape(arg.trim())
                    ));
                    out.push_str(&blocks_owned(&body, base + j, ctx));
                }
                "literalinclude" => {
                    let target = arg.trim();
                    let resolved = if let Some(rel) = target.strip_prefix('/') {
                        ctx.source_root.as_ref().map(|r| r.join(rel))
                    } else {
                        ctx.file
                            .as_ref()
                            .and_then(|f| f.parent())
                            .map(|d| d.join(target))
                    };
                    let unsupported: Vec<&str> = body
                        .iter()
                        .map(|l| l.trim())
                        .filter(|l| l.starts_with(':') && !l.starts_with(":language:"))
                        .collect();
                    if !unsupported.is_empty() {
                        ctx.warnings.push((
                            base + i + 1,
                            format!(
                                "literalinclude options not supported: {}",
                                unsupported.join(" ")
                            ),
                        ));
                    }
                    let lang = body
                        .iter()
                        .map(|l| l.trim())
                        .find_map(|l| l.strip_prefix(":language:"))
                        .map(|l| l.trim().to_owned())
                        .or_else(|| {
                            std::path::Path::new(target)
                                .extension()
                                .map(|e| e.to_string_lossy().into_owned())
                        })
                        .unwrap_or_default();
                    // Containment: only files under include_root (the
                    // project root by default) may embed — documentation
                    // sources must not be able to publish arbitrary
                    // readable files.
                    let joined = resolved.clone();
                    let resolved = resolved.and_then(|p| p.canonicalize().ok());
                    let outside = resolved.as_ref().is_some_and(|p| {
                        !ctx.include_root
                            .as_ref()
                            .is_some_and(|root| p.starts_with(root))
                    });
                    if outside {
                        let p = resolved.as_ref().expect("outside implies resolved");
                        ctx.warnings.push((
                            base + i + 1,
                            format!(
                                "literalinclude `{target}` resolves to {} — outside the \
                                 include root; set [guide] include_root to allow it",
                                p.display()
                            ),
                        ));
                    } else {
                        match resolved
                            .as_ref()
                            .and_then(|p| std::fs::read_to_string(p).ok())
                        {
                            Some(code) => {
                                let inner = crate::render::highlight::block(&code, &lang)
                                    .unwrap_or_else(|| html_escape(code.trim_end()));
                                out.push_str(&format!(
                                    "<pre><code class=\"language-{}\">{}</code></pre>\n",
                                    html_escape(&lang),
                                    inner
                                ));
                            }
                            None => {
                                ctx.warnings.push((
                                    base + i + 1,
                                    format!(
                                        "literalinclude: cannot read `{target}`{}",
                                        match resolved.as_ref().or(joined.as_ref()) {
                                            Some(p) => format!(" (resolved to {})", p.display()),
                                            None => String::new(),
                                        }
                                    ),
                                ));
                            }
                        }
                    }
                }
                "highlight" => {
                    ctx.default_lang = match arg.trim() {
                        "" | "none" | "text" => None,
                        lang => Some(lang.to_owned()),
                    };
                }
                "list-table" => {
                    out.push_str(&list_table(&arg, &body, ctx));
                }
                "only" => {
                    // We are the HTML builder: `.. only:: html` bodies
                    // belong in the output (dropping them silently lost
                    // real content); non-html builders' bodies are
                    // intentionally excluded, not a rendering gap. Only
                    // plain conjunctions are evaluated — a negation
                    // makes us conservatively skip.
                    let tokens: Vec<&str> = arg.split_whitespace().collect();
                    if tokens.contains(&"html") && !tokens.contains(&"not") {
                        out.push_str(&blocks_owned(&body, base + j, ctx));
                    }
                }
                "rubric" => {
                    out.push_str(&format!("<h3>{}</h3>\n", inline(&arg, ctx)));
                }
                _ if DROP_DIRECTIVES.contains(&name.as_str()) => {}
                _ => {
                    ctx.warnings
                        .push((base + i + 1, format!("unsupported rst directive `{name}`")));
                    out.push_str(&blocks_owned(&body, base + j, ctx));
                }
            }
            i = end;
            continue;
        }

        // Comments, link targets, substitutions.
        if stripped == ".." || stripped.starts_with(".. ") || stripped.starts_with("..\t") {
            i = block_end(i + 1, indent_of(line));
            continue;
        }

        // Grid table.
        if is_grid_line(line) && stripped.starts_with('+') {
            let mut j = i;
            while j < n && is_grid_line(lines[j]) {
                j += 1;
            }
            let region = dedent(&lines[i..j]);
            out.push_str(&grid_table(&region, ctx));
            i = j;
            continue;
        }

        // Line block: lines of `| text` (or a bare `|` spacer).
        if stripped == "|" || stripped.starts_with("| ") {
            let mut parts: Vec<String> = Vec::new();
            while i < n {
                let t = lines[i].trim();
                if t == "|" {
                    parts.push(String::new());
                } else if let Some(rest) = t.strip_prefix("| ") {
                    parts.push(inline(rest, ctx));
                } else {
                    break;
                }
                i += 1;
            }
            while parts.last().is_some_and(|p| p.is_empty()) {
                parts.pop();
            }
            if !parts.is_empty() {
                out.push_str(&format!(
                    "<p>{}</p>
",
                    parts.join("<br>")
                ));
            }
            continue;
        }

        // Simple table: border of two or more '=' runs.
        if SIMPLE_BORDER.is_match(stripped) {
            let mut j = i;
            while j < n && !lines[j].trim().is_empty() {
                j += 1;
            }
            let region = dedent(&lines[i..j]);
            if region.len() >= 3 && SIMPLE_BORDER.is_match(region.last().unwrap().trim()) {
                out.push_str(&simple_table(&region, ctx));
                i = j;
                continue;
            }
            // Fall through: not actually a table.
        }

        // Lists.
        if BULLET.is_match(line) || ENUM.is_match(line) {
            let ordered = ENUM.is_match(line);
            let (html, next) = list(lines, i, base, ordered, ctx);
            out.push_str(&html);
            i = next;
            continue;
        }

        // Indented block with no marker: block quote.
        if indent_of(line) > 0 {
            let end = block_end(i, 0);
            let body = dedent(&lines[i..end]);
            out.push_str("<blockquote>\n");
            out.push_str(&blocks_owned(&body, base + i, ctx));
            out.push_str("</blockquote>\n");
            i = end;
            continue;
        }

        // Paragraph, possibly introducing a literal block via `::`.
        let mut para: Vec<String> = Vec::new();
        let mut literal_next = false;
        while i < n {
            let l = lines[i];
            let t = l.trim();
            if t.is_empty()
                || DIRECTIVE.is_match(l)
                || t.starts_with(".. ")
                || is_grid_line(l)
                || BULLET.is_match(l)
                || ENUM.is_match(l)
                || (i + 1 < n
                    && adornment_char(lines[i + 1]).is_some()
                    && indent_of(l) == 0
                    && !para.is_empty())
            {
                break;
            }
            if t == "::" {
                literal_next = true;
                i += 1;
                break;
            }
            if let Some(lead) = t.strip_suffix("::") {
                para.push(format!("{}:", lead.trim_end()));
                literal_next = true;
                i += 1;
                break;
            }
            para.push(t.to_owned());
            i += 1;
        }
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", inline(&para.join(" "), ctx)));
        }
        if literal_next {
            while i < n && lines[i].trim().is_empty() {
                i += 1;
            }
            let end = block_end(i, 0);
            if end > i {
                let body = dedent(&lines[i..end]);
                let text = body.join("\n");
                let text = text.trim_end();
                let inner = ctx
                    .default_lang
                    .clone()
                    .and_then(|l| crate::render::highlight::block(&format!("{text}\n"), &l))
                    .unwrap_or_else(|| html_escape(text));
                out.push_str(&format!("<pre><code>{inner}</code></pre>\n"));
                i = end;
            }
        }
        // Termination guarantee: no branch may leave the cursor parked.
        if i == iter_start {
            ctx.warnings.push((
                base + i + 1,
                format!("rst parser could not consume line: {stripped}"),
            ));
            i += 1;
        }
    }
    out
}

fn admonition(
    class: &str,
    name: &str,
    args: &[String],
    body: &[String],
    base: usize,
    ctx: &mut Ctx,
) -> String {
    let mut title = name.to_owned();
    if let Some(c) = title.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    let mut inner = String::new();
    if let Some(first) = args.first() {
        inner.push_str(&format!("<p>{}</p>\n", inline(first, ctx)));
    }
    inner.push_str(&blocks_owned(body, base, ctx));
    format!("<div class=\"{class}\"><strong>{title}:</strong>\n{inner}</div>\n")
}

fn list(
    lines: &[&str],
    start: usize,
    base_line: usize,
    ordered: bool,
    ctx: &mut Ctx,
) -> (String, usize) {
    let n = lines.len();
    let marker = if ordered { &*ENUM } else { &*BULLET };
    let base = indent_of(lines[start]);
    let mut items: Vec<String> = Vec::new();
    let mut i = start;
    while i < n {
        let line = lines[i];
        if line.trim().is_empty() {
            // A blank ends the list only if what follows dedents.
            let mut k = i;
            while k < n && lines[k].trim().is_empty() {
                k += 1;
            }
            if k < n
                && (indent_of(lines[k]) > base
                    || (marker.is_match(lines[k]) && indent_of(lines[k]) == base))
            {
                i = k;
                continue;
            }
            break;
        }
        if marker.is_match(line) && indent_of(line) == base {
            // Head of a new item: text after the marker.
            let after = line.trim_start();
            let text_start = after
                .find(char::is_whitespace)
                .map(|p| after[p..].trim_start().to_owned())
                .unwrap_or_default();
            let head_line = i;
            let mut item_lines: Vec<&str> = Vec::new();
            i += 1;
            while i < n && (lines[i].trim().is_empty() || indent_of(lines[i]) > base) {
                item_lines.push(lines[i]);
                i += 1;
            }
            while item_lines.last().is_some_and(|l| l.trim().is_empty()) {
                item_lines.pop();
            }
            let mut body = vec![text_start];
            body.extend(dedent(&item_lines));
            items.push(blocks_owned(&body, base_line + head_line, ctx));
        } else if indent_of(line) <= base {
            break;
        } else {
            i += 1; // defensive: swallow stray deeper line
        }
    }
    let tag = if ordered { "ol" } else { "ul" };
    let mut html = format!("<{tag}>\n");
    for it in items {
        html.push_str(&format!("<li>{}</li>\n", it.trim_end()));
    }
    html.push_str(&format!("</{tag}>\n"));
    (html, i)
}

// ----- tables -------------------------------------------------------------

/// Cell fragments within one row: wrapped lines join with a space,
/// blank-separated groups join with a line break.
fn join_fragments(frags: &[String]) -> String {
    let mut groups: Vec<Vec<&str>> = vec![Vec::new()];
    for f in frags {
        if f.trim().is_empty() {
            if !groups.last().unwrap().is_empty() {
                groups.push(Vec::new());
            }
        } else {
            groups.last_mut().unwrap().push(f.trim());
        }
    }
    // Groups are joined with a control-char sentinel; table_html renders
    // each group through inline() and joins with <br> (which would
    // otherwise be escaped).
    groups
        .iter()
        .filter(|g| !g.is_empty())
        .map(|g| g.join(" "))
        .collect::<Vec<_>>()
        .join("\u{3}")
}

fn inline_cell(cell: &str, ctx: &mut Ctx) -> String {
    cell.split('\u{3}')
        .map(|g| inline(g, ctx))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn table_html(header: &[Vec<String>], rows: &[Vec<String>], ctx: &mut Ctx) -> String {
    let mut html = String::from("<table class=\"rows doctable\">\n");
    if !header.is_empty() {
        html.push_str("<thead>\n");
        for row in header {
            html.push_str("<tr>");
            for cell in row {
                html.push_str(&format!("<th>{}</th>", inline_cell(cell, ctx)));
            }
            html.push_str("</tr>\n");
        }
        html.push_str("</thead>\n");
    }
    html.push_str("<tbody>\n");
    for row in rows {
        html.push_str("<tr>");
        for cell in row {
            html.push_str(&format!("<td>{}</td>", inline_cell(cell, ctx)));
        }
        html.push_str("</tr>\n");
    }
    html.push_str("</tbody>\n</table>\n");
    html
}

fn grid_table(region: &[String], ctx: &mut Ctx) -> String {
    let border: Vec<char> = region[0].chars().collect();
    let plus: Vec<usize> = border
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '+')
        .map(|(i, _)| i)
        .collect();
    if plus.len() < 2 {
        return format!("<p>{}</p>\n", inline(&region.join(" "), ctx));
    }
    let ncols = plus.len() - 1;

    let mut header: Vec<Vec<String>> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut frags: Vec<Vec<String>> = vec![Vec::new(); ncols];
    let mut have_frags = false;

    for line in region {
        let t = line.trim_end();
        if t.starts_with('+') {
            if have_frags {
                rows.push(frags.iter().map(|f| join_fragments(f)).collect());
                frags = vec![Vec::new(); ncols];
                have_frags = false;
            }
            if t.contains('=') && !rows.is_empty() && header.is_empty() {
                header = std::mem::take(&mut rows);
            }
        } else if t.starts_with('|') {
            let chars: Vec<char> = t.chars().collect();
            for k in 0..ncols {
                let a = (plus[k] + 1).min(chars.len());
                let b = plus[k + 1].min(chars.len());
                let cell: String = chars[a..b].iter().collect();
                frags[k].push(cell);
            }
            have_frags = true;
        }
    }
    if have_frags {
        rows.push(frags.iter().map(|f| join_fragments(f)).collect());
    }
    table_html(&header, &rows, ctx)
}

fn simple_table(region: &[String], ctx: &mut Ctx) -> String {
    let border = &region[0];
    // Column ranges: runs of '=' in the border; the last column extends
    // to end of line.
    let chars: Vec<char> = border.chars().collect();
    let mut cols: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        if chars[k] == '=' {
            let s = k;
            while k < chars.len() && chars[k] == '=' {
                k += 1;
            }
            cols.push((s, k));
        } else {
            k += 1;
        }
    }
    if let Some(last) = cols.last_mut() {
        last.1 = usize::MAX;
    }

    let border_idx: Vec<usize> = region
        .iter()
        .enumerate()
        .filter(|(_, l)| SIMPLE_BORDER.is_match(l.trim()))
        .map(|(i, _)| i)
        .collect();

    let slice_row = |line: &str| -> Vec<String> {
        let lc: Vec<char> = line.chars().collect();
        cols.iter()
            .map(|(a, b)| {
                let a = (*a).min(lc.len());
                let b = (*b).min(lc.len());
                lc[a..b].iter().collect::<String>().trim().to_owned()
            })
            .collect()
    };

    let (header_lines, body_lines): (&[String], &[String]) = if border_idx.len() >= 3 {
        (
            &region[border_idx[0] + 1..border_idx[1]],
            &region[border_idx[1] + 1..*border_idx.last().unwrap()],
        )
    } else {
        (&[], &region[border_idx[0] + 1..*border_idx.last().unwrap()])
    };
    let header: Vec<Vec<String>> = header_lines.iter().map(|l| slice_row(l)).collect();
    let rows: Vec<Vec<String>> = body_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| slice_row(l))
        .collect();
    table_html(&header, &rows, ctx)
}

// ----- inline markup ------------------------------------------------------

fn code_span(text: &str, ctx: &mut Ctx) -> String {
    let escaped = html_escape(text);
    let name = text.trim().trim_end_matches("()");
    if let Some(resolve) = ctx.resolve
        && let Some(href) = resolve(name)
    {
        return format!("<a class=\"apilink\" href=\"{href}\"><code>{escaped}</code></a>");
    }
    format!("<code>{escaped}</code>")
}

fn role_is_codeish(role: &str) -> bool {
    let last = role.rsplit(':').next().unwrap_or(role);
    role.contains(':') || CODE_ROLES.contains(&last)
}

fn inline(text: &str, ctx: &mut Ctx) -> String {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;

    let find_from = |pat: &str, from: usize| -> Option<usize> {
        text.get(from..).and_then(|s| s.find(pat)).map(|p| p + from)
    };

    while i < n {
        // Backslash escape.
        if b[i] == b'\\' && i + 1 < n {
            let c = text[i + 1..].chars().next().unwrap();
            out.push_str(&html_escape(&c.to_string()));
            i += 1 + c.len_utf8();
            continue;
        }
        // ``literal``
        if text[i..].starts_with("``")
            && let Some(close) = find_from("``", i + 2)
        {
            out.push_str(&code_span(&text[i + 2..close], ctx));
            i = close + 2;
            continue;
        }
        // :role:`content` or :role:`text <target>`
        if b[i] == b':' {
            let rest = &text[i + 1..];
            if let Some(tick) = rest.find('`') {
                let role = &rest[..tick];
                if tick > 1
                    && role.ends_with(':')
                    && role[..role.len() - 1].chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, ':' | '+' | '-' | '_' | '.')
                    })
                {
                    let role_name = &role[..role.len() - 1];
                    let content_start = i + 1 + tick + 1;
                    if let Some(close) = find_from("`", content_start) {
                        let inner = &text[content_start..close];
                        let (display, target) = match inner.rfind('<') {
                            Some(lt) if inner.ends_with('>') => {
                                (inner[..lt].trim(), inner[lt + 1..inner.len() - 1].trim())
                            }
                            _ => (inner.trim(), inner.trim()),
                        };
                        let display = if display.is_empty() {
                            inner.trim()
                        } else {
                            display
                        };
                        let explicit_text = display != target;
                        if role_name == "doc" && ctx.doc_pages.is_some() {
                            // Documents are flat stem-keyed pages; take
                            // the last path segment.
                            let stem = target
                                .trim_start_matches('/')
                                .rsplit('/')
                                .next()
                                .unwrap_or(target);
                            match ctx.doc_pages.as_ref().and_then(|d| d.get(stem)) {
                                Some(title) => {
                                    let label = if explicit_text { display } else { title };
                                    out.push_str(&format!(
                                        "<a href=\"{}.html\">{}</a>",
                                        html_escape(stem),
                                        html_escape(label)
                                    ));
                                }
                                None => {
                                    ctx.warnings.push((
                                        ctx.line,
                                        format!(":doc: target `{target}` matches no guide page"),
                                    ));
                                    out.push_str(&format!("<em>{}</em>", html_escape(display)));
                                }
                            }
                        } else if role_name == "ref" && ctx.ref_targets.is_some() {
                            // Exact label first; then the Sphinx
                            // autosectionlabel form `docname:Section`,
                            // keyed by page stem + lowercased title.
                            let hit = ctx.ref_targets.as_ref().and_then(|r| {
                                r.get(target).or_else(|| {
                                    let (doc, section) = target.split_once(':')?;
                                    let stem = doc.rsplit('/').next()?;
                                    r.get(&format!("{stem}:{}", section.to_lowercase()))
                                })
                            });
                            match hit {
                                Some((url, title)) => {
                                    let label = if explicit_text {
                                        display
                                    } else {
                                        title.as_deref().unwrap_or(display)
                                    };
                                    out.push_str(&format!(
                                        "<a href=\"{}\">{}</a>",
                                        html_escape(url),
                                        html_escape(label)
                                    ));
                                }
                                None => {
                                    ctx.warnings.push((
                                        ctx.line,
                                        format!(
                                            ":ref: target `{target}` is not defined in the guide"
                                        ),
                                    ));
                                    out.push_str(&format!("<em>{}</em>", html_escape(display)));
                                }
                            }
                        } else if role_name == "rfc" {
                            out.push_str(&format!(
                                "<a href=\"https://www.rfc-editor.org/rfc/rfc{0}.html\">RFC {0}</a>",
                                html_escape(display)
                            ));
                        } else if role_is_codeish(role_name) {
                            // An explicit xref role is a declared API
                            // reference; failing to resolve it is doc
                            // rot, unlike a bare ``code`` literal.
                            if ctx
                                .resolve
                                .is_some_and(|r| r(display.trim().trim_end_matches("()")).is_none())
                            {
                                ctx.warnings.push((
                                    ctx.line,
                                    format!(":{role_name}: reference `{display}` does not resolve"),
                                ));
                            }
                            out.push_str(&code_span(display, ctx));
                        } else {
                            out.push_str(&format!("<em>{}</em>", html_escape(display)));
                        }
                        i = close + 1;
                        continue;
                    }
                }
            }
        }
        // `text <url>`_ / `ref`_ / `interpreted`
        if b[i] == b'`'
            && let Some(close) = find_from("`", i + 1)
        {
            let inner = &text[i + 1..close];
            let after = &text[close + 1..];
            let underscores = after.chars().take_while(|c| *c == '_').count();
            if underscores > 0 {
                match (inner.rfind('<'), inner.ends_with('>')) {
                    (Some(lt), true) => {
                        let label = inner[..lt].trim();
                        let url = &inner[lt + 1..inner.len() - 1];
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            html_escape(url),
                            html_escape(label)
                        ));
                    }
                    _ => out.push_str(&html_escape(inner.trim())),
                }
                i = close + 1 + underscores;
            } else {
                out.push_str(&format!("<em>{}</em>", html_escape(inner)));
                i = close + 1;
            }
            continue;
        }
        // **strong** / *emphasis*
        if text[i..].starts_with("**")
            && let Some(close) = find_from("**", i + 2)
        {
            out.push_str(&format!(
                "<strong>{}</strong>",
                html_escape(&text[i + 2..close])
            ));
            i = close + 2;
            continue;
        }
        if b[i] == b'*'
            && let Some(close) = find_from("*", i + 1)
        {
            out.push_str(&format!("<em>{}</em>", html_escape(&text[i + 1..close])));
            i = close + 1;
            continue;
        }
        // Bare URLs.
        if text[i..].starts_with("http://") || text[i..].starts_with("https://") {
            let end = text[i..]
                .find(|c: char| c.is_whitespace() || matches!(c, '>' | ')' | '"'))
                .map(|p| p + i)
                .unwrap_or(n);
            let url = text[i..end].trim_end_matches(['.', ',', ';', ':']);
            out.push_str(&format!("<a href=\"{0}\">{0}</a>", html_escape(url)));
            i += url.len();
            continue;
        }
        let c = text[i..].chars().next().unwrap();
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(src: &str) -> RstOutput {
        render(src, &RstOptions::default())
    }

    #[test]
    fn default_lang_literal_blocks() {
        let out = render(
            "T\n=\n\nExample::\n\n   int x = 1;\n\n.. highlight:: none\n\nPlain::\n\n   just text\n",
            &RstOptions {
                default_lang: Some("cpp"),
                ..RstOptions::default()
            },
        );
        assert!(out.html.contains("hl-"), "literal block not highlighted");
        let plain = out.html.rfind("just text").unwrap();
        assert!(
            !out.html[plain - 60..].contains("hl-"),
            "highlight:: none ignored"
        );
    }

    #[test]
    fn headings_paragraphs_inline() {
        let out = go(
            "Title\n=====\n\nUses ``HashFunction`` with *style* and **force**.\n\nSection\n-------\n\nSee `docs <https://x.example>`_ and :cpp:class:`BigInt` and :rfc:`5869`.\n",
        );
        assert_eq!(out.title.as_deref(), Some("Title"));
        assert!(out.html.contains("<h1>Title</h1>"));
        assert!(out.html.contains("<h2>Section</h2>"));
        assert!(out.html.contains("<code>HashFunction</code>"));
        assert!(out.html.contains("<em>style</em>"));
        assert!(out.html.contains("<strong>force</strong>"));
        assert!(out.html.contains("<a href=\"https://x.example\">docs</a>"));
        assert!(out.html.contains("<code>BigInt</code>"));
        assert!(
            out.html
                .contains("rfc-editor.org/rfc/rfc5869.html\">RFC 5869</a>")
        );
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn directives_and_lists() {
        let out = go(
            "T\n=\n\n.. code-block:: cpp\n\n   int x = 1;\n\n.. note::\n   Careful.\n\n.. cpp:function:: void f(int a)\n\n   Does things.\n\n- first item\n  continued\n- second\n\n.. mystery::\n   kept\n",
        );
        assert!(out.html.contains("class=\"language-cpp\""));
        assert!(out.html.contains("hl-"), "code not highlighted");
        assert!(
            out.html
                .contains("<div class=\"note\"><strong>Note:</strong>")
        );
        assert!(out.html.contains("<pre class=\"sig\">void f(int a)</pre>"));
        assert!(out.html.contains("<div class=\"member\">"));
        assert!(out.html.contains("<li><p>first item continued</p>"));
        assert!(out.html.contains("kept"));
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(
            out.warnings[0].message,
            "unsupported rst directive `mystery`"
        );
        assert!(out.warnings[0].line > 0);
    }

    #[test]
    fn embed_hook_and_sig_capture() {
        let embed = |req: &EmbedRequest| -> Option<String> {
            (req.name == "Widget::frob").then(|| "<div class=\"member\">FROB</div>".to_owned())
        };
        let out = render(
            "T\n=\n\n.. udg:member:: Widget::frob\n\n.. udg:member:: Nope\n\n.. cpp:function:: int f(int a, int b)\n\n   doc\n",
            &RstOptions {
                embed: Some(&embed),
                ..RstOptions::default()
            },
        );
        assert!(out.html.contains("FROB"));
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].message.contains("cannot resolve `Nope`"));
        assert_eq!(out.sig_directives.len(), 1);
        assert_eq!(out.sig_directives[0].1, crate::ir::Language::Cpp);
        assert_eq!(out.sig_directives[0].2, "int f(int a, int b)");
    }

    #[test]
    fn autodoc_directives() {
        use std::cell::RefCell;
        let seen: RefCell<Vec<(String, Option<Vec<String>>)>> = RefCell::new(Vec::new());
        let embed = |req: &EmbedRequest| -> Option<String> {
            assert_eq!(req.lang, Some(crate::ir::Language::Python));
            seen.borrow_mut()
                .push((req.name.to_owned(), req.members.clone()));
            match req.name {
                "version_major" => Some("<div>VMAJOR</div>".to_owned()),
                "botan3.RandomNumberGenerator" => Some("<div>RNG</div>".to_owned()),
                _ => None,
            }
        };
        let out = render(
            "T\n=\n\n.. py:module:: botan3\n\n.. autofunction:: version_major\n\n\
             .. autoclass:: botan3.RandomNumberGenerator\n   :members:\n   :undoc-members:\n\n\
             .. autodata:: nope\n",
            &RstOptions {
                embed: Some(&embed),
                ..RstOptions::default()
            },
        );
        assert!(out.html.contains("VMAJOR"));
        assert!(out.html.contains("RNG"));
        // Bare names try as spelled, then qualified by the py:module
        // context; already-qualified names resolve on the first try.
        let seen = seen.borrow();
        assert_eq!(seen[0].0, "version_major");
        assert_eq!(
            seen[1],
            ("botan3.RandomNumberGenerator".to_owned(), Some(vec![]))
        );
        // `nope` fails as spelled and as `botan3.nope` -> one warning.
        let unresolved: Vec<_> = out
            .warnings
            .iter()
            .filter(|w| w.message.contains("cannot resolve `nope`"))
            .collect();
        assert_eq!(unresolved.len(), 1);
        // Unknown options warn instead of silently vanishing.
        assert!(
            out.warnings
                .iter()
                .any(|w| w.message.contains(":undoc-members:` ignored"))
        );
        // Autodoc directives are machine-resolved: no drift-lint capture.
        assert!(out.sig_directives.is_empty());
    }

    #[test]
    fn ref_and_doc_roles_resolve() {
        let mut targets = HashMap::new();
        targets.insert(
            "tls_client".to_owned(),
            ("tls.html".to_owned(), Some("TLS Client".to_owned())),
        );
        targets.insert(
            "pubkey:rsa".to_owned(),
            ("pubkey.html".to_owned(), Some("RSA".to_owned())),
        );
        let mut docs = HashMap::new();
        docs.insert("building".to_owned(), "Building the Library".to_owned());
        let out = render(
            "T\n=\n\nSee :ref:`tls_client`, :ref:`api_ref/pubkey:RSA`, :doc:`building`, \
             :ref:`missing`, :doc:`gone`.\n",
            &RstOptions {
                ref_targets: Some(&targets),
                doc_pages: Some(&docs),
                ..RstOptions::default()
            },
        );
        // Explicit label (bare form shows the section title), Sphinx
        // autosectionlabel form, and :doc: title lookup.
        assert!(
            out.html.contains("<a href=\"tls.html\">TLS Client</a>"),
            "{}",
            out.html
        );
        assert!(out.html.contains("<a href=\"pubkey.html\">RSA</a>"));
        assert!(
            out.html
                .contains("<a href=\"building.html\">Building the Library</a>")
        );
        // Unresolvable targets warn with the paragraph's line.
        assert_eq!(out.warnings.len(), 2);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.message.contains(":ref: target `missing`") && w.line == 4)
        );
        assert!(
            out.warnings
                .iter()
                .any(|w| w.message.contains(":doc: target `gone`"))
        );
    }

    #[test]
    fn literalinclude_stays_inside_include_root() {
        let base = std::env::temp_dir().join(format!("udg-rst-inc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("doc")).unwrap();
        std::fs::write(base.join("doc/ok.txt"), "inside\n").unwrap();
        std::fs::write(base.join("secret.txt"), "outside\n").unwrap();
        let file = base.join("doc/page.rst");
        std::fs::write(&file, "").unwrap();

        let render_with = |target: &str, root: &std::path::Path| {
            render(
                &format!("T\n=\n\n.. literalinclude:: {target}\n"),
                &RstOptions {
                    file: Some(&file),
                    source_root: Some(&base.join("doc")),
                    include_root: Some(root),
                    ..RstOptions::default()
                },
            )
        };
        // Inside the root: embeds.
        let out = render_with("ok.txt", &base.join("doc"));
        assert!(out.html.contains("inside"), "{}", out.html);
        // Escaping the root: refused with a warning.
        let out = render_with("../secret.txt", &base.join("doc"));
        assert!(!out.html.contains("outside"));
        assert!(
            out.warnings
                .iter()
                .any(|w| w.message.contains("outside the include root"))
        );
        // A wider configured root allows it.
        let out = render_with("../secret.txt", &base);
        assert!(out.html.contains("outside"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn only_html_renders_other_builders_drop() {
        let out = go(
            "T\n=\n\n.. only:: html\n\n   Kept content.\n\n.. only:: html and website\n\n   Also kept.\n\n.. only:: latex\n\n   Dropped.\n\n.. only:: not html\n\n   Dropped too.\n",
        );
        assert!(out.html.contains("Kept content."));
        assert!(out.html.contains("Also kept."));
        assert!(!out.html.contains("Dropped"));
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn list_table_renders() {
        let out = go(
            "T\n=\n\n.. list-table:: Algos\n   :header-rows: 1\n   :widths: 30 70\n\n   * - Field\n     - Notes\n   * - ``with_mac``\n     - Legacy; widest\n       compatibility\n   * - x\n     - -\n",
        );
        assert!(
            out.warnings.is_empty(),
            "{:?}",
            out.warnings.iter().map(|w| &w.message).collect::<Vec<_>>()
        );
        assert!(out.html.contains("<caption>Algos</caption>"));
        assert!(out.html.contains("<th>Field</th><th>Notes</th>"));
        assert!(
            out.html
                .contains("<td><code>with_mac</code></td><td>Legacy; widest compatibility</td>"),
            "{}",
            out.html
        );
        // A cell whose content is a literal dash.
        assert!(out.html.contains("<td>-</td>"));
    }

    #[test]
    fn toctree_capture() {
        let out = go(
            "T\n=\n\n.. toctree::\n   :maxdepth: 1\n\n   building\n   api_ref/contents\n   Custom <cli>\n",
        );
        let entries: Vec<&str> = out.toctree.iter().map(|(_, e)| e.as_str()).collect();
        assert_eq!(entries, vec!["building", "api_ref/contents", "cli"]);
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn literal_block() {
        let out = go("Example::\n\n   $ make\n   $ make check\n\nAfter.\n");
        assert!(out.html.contains("<p>Example:</p>"));
        assert!(
            out.html
                .contains("<pre><code>$ make\n$ make check</code></pre>")
        );
        assert!(out.html.contains("<p>After.</p>"));
    }

    #[test]
    fn simple_table() {
        let out =
            go("====== ======\nA Col  B Col\n====== ======\n1      x\n2      y\n====== ======\n");
        assert!(
            out.html.contains("<th>A Col</th><th>B Col</th>"),
            "{}",
            out.html
        );
        assert!(out.html.contains("<td>1</td><td>x</td>"));
        assert!(out.html.contains("<td>2</td><td>y</td>"));
    }

    #[test]
    fn grid_table_multiline_cells() {
        let out = go(
            "+-----+----------+\n| Alg | Ext      |\n+=====+==========+\n| AES | VAES     |\n|     |          |\n|     | AES-NI   |\n+-----+----------+\n",
        );
        assert!(
            out.html.contains("<th>Alg</th><th>Ext</th>"),
            "{}",
            out.html
        );
        assert!(out.html.contains("<td>AES</td><td>VAES<br>AES-NI</td>"));
    }
}
