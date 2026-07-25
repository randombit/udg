//! Serde view models passed to templates. All `*_html` fields hold
//! pre-rendered, safe HTML (templates apply `|safe`); everything else is
//! plain text that templates escape.

use crate::ir::DocBlock;
use serde::Serialize;

use crate::render::md::{Md, Resolver};

#[derive(Serialize)]
pub struct Crumb {
    pub name: String,
    /// Site-root-relative URL; empty string means "no link".
    pub url: String,
}

#[derive(Serialize, Clone)]
pub struct Row {
    pub name: String,
    pub url: String,
    pub summary_html: String,
    pub since: Option<String>,
    pub deprecated: bool,
}

#[derive(Serialize)]
pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
}

#[derive(Serialize, Default)]
pub struct DocsView {
    pub body_html: Option<String>,
    pub params: Vec<DocParamView>,
    pub returns_html: Option<String>,
    pub throws_html: Vec<String>,
    pub warnings_html: Vec<String>,
    pub notes_html: Vec<String>,
    /// Pre-rendered `<code>`/`<a>` fragments (resolved where possible).
    pub see_also_html: Vec<String>,
    pub deprecated_html: Option<String>,
}

#[derive(Serialize)]
pub struct BindingLink {
    pub label: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct DocParamView {
    pub name: String,
    pub html: String,
}

#[derive(Serialize)]
pub struct MemberView {
    pub anchor: String,
    /// Escaped signature HTML with linked type names.
    pub sig_html: String,
    pub since: Option<String>,
    pub deprecated: bool,
    pub docs: Option<DocsView>,
    pub src_url: Option<String>,
}

pub fn docs_view(
    d: &DocBlock,
    deprecated_msg: Option<&str>,
    md: &Md,
    res: Option<Resolver>,
) -> DocsView {
    DocsView {
        body_html: d.body.as_deref().map(|b| md.html_with(b, res)),
        params: d
            .params
            .iter()
            .map(|p| DocParamView {
                name: p.name.clone(),
                html: md.inline_with(&p.text, res),
            })
            .collect(),
        returns_html: d.returns.as_deref().map(|r| md.inline_with(r, res)),
        throws_html: d.throws.iter().map(|t| md.inline_with(t, res)).collect(),
        warnings_html: d.warnings.iter().map(|t| md.inline_with(t, res)).collect(),
        notes_html: d.notes.iter().map(|t| md.inline_with(t, res)).collect(),
        see_also_html: d
            .see_also
            .iter()
            .map(|s| {
                let code = format!("<code>{}</code>", crate::render::html_escape(s));
                match res.and_then(|r| r(s.trim().trim_end_matches("()"))) {
                    Some(href) => format!("<a class=\"apilink\" href=\"{href}\">{code}</a>"),
                    None => code,
                }
            })
            .collect(),
        deprecated_html: deprecated_msg
            .filter(|m| !m.is_empty())
            .map(|m| md.inline_with(m, res)),
    }
}
