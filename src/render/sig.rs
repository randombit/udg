//! Reconstruction of display signatures from structured IR.
//!
//! Signatures are built as chunk lists so rendering knows which spans
//! are *type positions*: only those autolink. Linkifying the whole
//! string would turn parameter names into confidently wrong links
//! (`reason` resolving to some unrelated field).

use crate::ir::{Item, ItemKind, Language};

/// One piece of a signature. `ty` spans may autolink; names,
/// punctuation, qualifiers, and default values never do.
pub struct Chunk {
    pub text: String,
    pub ty: bool,
}

fn no(text: impl Into<String>) -> Chunk {
    Chunk {
        text: text.into(),
        ty: false,
    }
}

fn ty(text: impl Into<String>) -> Chunk {
    Chunk {
        text: text.into(),
        ty: true,
    }
}

fn flatten(chunks: &[Chunk]) -> String {
    chunks.iter().map(|c| c.text.as_str()).collect()
}

/// Language-dispatching entry points used by the page builder.
pub fn for_function(item: &Item) -> String {
    flatten(&for_function_chunks(item))
}

pub fn for_leaf(item: &Item) -> String {
    flatten(&for_leaf_chunks(item))
}

pub fn for_function_chunks(item: &Item) -> Vec<Chunk> {
    match item.lang {
        Language::Python => py_function(item),
        _ => function(item),
    }
}

pub fn for_record_chunks(item: &Item) -> Vec<Chunk> {
    match item.lang {
        Language::Python => py_record(item),
        _ => record(item),
    }
}

pub fn for_leaf_chunks(item: &Item) -> Vec<Chunk> {
    match (item.lang, item.kind) {
        (Language::Python, ItemKind::Variable | ItemKind::Field) => py_variable(item),
        (Language::Python, _) => py_function(item),
        _ => leaf(item),
    }
}

/// Join per-parameter chunk lists into the final signature, one-lined
/// when it fits (matching the historical text layout exactly).
fn with_params(
    mut head: Vec<Chunk>,
    params: Vec<Vec<Chunk>>,
    tail: &str,
    indent: &str,
) -> Vec<Chunk> {
    let one_line = format!(
        "{}({}){tail}",
        flatten(&head),
        params
            .iter()
            .map(|p| flatten(p))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let width = one_line.lines().last().map(str::len).unwrap_or(0);
    let (open, sep, close) = if width <= 96 || params.len() <= 1 {
        ("(".to_owned(), ", ".to_owned(), format!("){tail}"))
    } else {
        (
            format!("(\n{indent}"),
            format!(",\n{indent}"),
            format!("\n){tail}"),
        )
    };
    head.push(no(open));
    for (i, p) in params.into_iter().enumerate() {
        if i > 0 {
            head.push(no(sep.clone()));
        }
        head.extend(p);
    }
    head.push(no(close));
    head
}

fn py_function(item: &Item) -> Vec<Chunk> {
    let Some(s) = &item.signature else {
        return vec![no(format!("def {}(...)", item.name))];
    };
    let mut head = Vec::new();
    if s.is_static {
        head.push(no("@staticmethod\n"));
    }
    head.push(no(format!("def {}", item.name)));
    let params: Vec<Vec<Chunk>> = s
        .params
        .iter()
        .map(|p| {
            let mut v = vec![no(p.name.clone().unwrap_or_default())];
            if !p.ty.is_empty() {
                v.push(no(": "));
                v.push(ty(p.ty.clone()));
            }
            if let Some(d) = &p.default {
                // PEP 8: spaces around `=` only when an annotation is present.
                v.push(no(format!(
                    "{}{d}",
                    if p.ty.is_empty() { "=" } else { " = " }
                )));
            }
            v
        })
        .collect();
    let tail = match s.return_type.as_deref() {
        Some(r) => format!(" -> {r}"),
        None => String::new(),
    };
    let mut out = with_params(head, params, &tail, "   ");
    // The return annotation is a type position: re-emit it as one.
    if let Some(r) = &s.return_type {
        let last = out.last_mut().expect("with_params emits a closer");
        last.text
            .truncate(last.text.len() - format!(" -> {r}").len());
        out.push(no(" -> "));
        out.push(ty(r.clone()));
    }
    out
}

fn py_record(item: &Item) -> Vec<Chunk> {
    let mut out = vec![no(format!("class {}", item.name))];
    if !item.bases.is_empty() {
        out.push(no("("));
        for (i, b) in item.bases.iter().enumerate() {
            if i > 0 {
                out.push(no(", "));
            }
            out.push(ty(b.clone()));
        }
        out.push(no(")"));
    }
    out
}

fn py_variable(item: &Item) -> Vec<Chunk> {
    let mut out = vec![no(item.name.clone())];
    if let Some(t) = &item.ty {
        out.push(no(": "));
        out.push(ty(t.clone()));
    }
    if let Some(v) = &item.value {
        out.push(no(format!(" = {v}")));
    }
    out
}

fn function(item: &Item) -> Vec<Chunk> {
    let Some(s) = &item.signature else {
        return vec![no(item.name.clone())];
    };
    let mut head = Vec::new();
    if let Some(t) = &s.template_params {
        head.push(no(format!("{t}\n")));
    }
    if s.is_static {
        head.push(no("static "));
    }
    if s.is_virtual {
        head.push(no("virtual "));
    }
    if let Some(r) = &s.return_type {
        head.push(ty(r.clone()));
        head.push(no(" "));
    }
    head.push(no(item.name.clone()));

    let params: Vec<Vec<Chunk>> = s
        .params
        .iter()
        .map(|p| {
            let mut v = vec![ty(p.ty.clone())];
            if let Some(n) = &p.name {
                v.push(no(format!(" {n}")));
            }
            if let Some(d) = &p.default {
                v.push(no(format!(" = {d}")));
            }
            v
        })
        .collect();

    let mut tail = String::new();
    if s.is_const {
        tail.push_str(" const");
    }
    if s.is_noexcept {
        tail.push_str(" noexcept");
    }
    if s.is_override {
        tail.push_str(" override");
    }
    if s.is_final {
        tail.push_str(" final");
    }
    if s.is_pure_virtual {
        tail.push_str(" = 0");
    }

    with_params(head, params, &tail, "   ")
}

fn record(item: &Item) -> Vec<Chunk> {
    let mut out = Vec::new();
    if let Some(t) = item
        .signature
        .as_ref()
        .and_then(|s| s.template_params.as_ref())
    {
        out.push(no(format!("{t}\n")));
    }
    out.push(no(match item.kind {
        ItemKind::Struct => "struct ",
        _ => "class ",
    }));
    out.push(no(item.name.clone()));
    if !item.bases.is_empty() {
        out.push(no(" : "));
        for (i, b) in item.bases.iter().enumerate() {
            if i > 0 {
                out.push(no(", "));
            }
            out.push(no("public "));
            out.push(ty(b.clone()));
        }
    }
    out
}

fn leaf(item: &Item) -> Vec<Chunk> {
    match item.kind {
        ItemKind::TypeAlias => vec![
            no(format!("using {} = ", item.name)),
            ty(item.ty.clone().unwrap_or_else(|| "/* unknown */".into())),
        ],
        ItemKind::Variable | ItemKind::Field => vec![
            ty(item.ty.clone().unwrap_or_else(|| "/* unknown */".into())),
            no(format!(" {}", item.name)),
        ],
        // A constant hoisted out of an unnamed enum.
        ItemKind::Enumerator => match &item.value {
            Some(v) => vec![no(format!("{} = {v}", item.name))],
            None => vec![no(item.name.clone())],
        },
        ItemKind::Macro => {
            let mut text = format!("#define {}", item.name);
            if let Some(sig) = &item.signature {
                let params: Vec<&str> = sig
                    .params
                    .iter()
                    .filter_map(|p| p.name.as_deref())
                    .collect();
                text.push('(');
                text.push_str(&params.join(", "));
                text.push(')');
            }
            if let Some(v) = &item.value {
                text.push(' ');
                text.push_str(v);
            }
            vec![no(text)]
        }
        _ => function(item),
    }
}
