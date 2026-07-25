//! Markdown rendering (comrak) for doc bodies, plus inline rendering
//! for one-line summaries, with optional autolinking of resolvable
//! code spans.

use comrak::Options;

/// Resolves a code-span mention (`PK_Signer`, `sign()`, …) to an href,
/// or None to leave it unlinked.
pub type Resolver<'r> = &'r dyn Fn(&str) -> Option<String>;

pub struct Md {
    options: Options<'static>,
}

impl Md {
    pub fn new() -> Self {
        let mut options = Options::default();
        options.extension.table = true;
        options.extension.strikethrough = true;
        options.extension.autolink = true;
        // Doc comments regularly contain `<T>`-ish text that is not HTML;
        // escape raw HTML rather than passing it through.
        options.render.escape = true;
        Md { options }
    }

    pub fn html(&self, text: &str) -> String {
        highlight_fenced_blocks(&comrak::markdown_to_html(text, &self.options))
    }

    pub fn html_with(&self, text: &str, resolve: Option<Resolver>) -> String {
        let html = self.html(text);
        match resolve {
            Some(r) => autolink_code_spans(&html, r),
            None => html,
        }
    }

    /// Render a single paragraph without the wrapping `<p>` tags, for
    /// item listings and summaries.
    pub fn inline(&self, text: &str) -> String {
        let html = self.html(text);
        let html = html.trim();
        html.strip_prefix("<p>")
            .and_then(|h| h.strip_suffix("</p>"))
            .map(str::to_owned)
            .unwrap_or_else(|| html.to_owned())
    }

    pub fn inline_with(&self, text: &str, resolve: Option<Resolver>) -> String {
        let html = self.inline(text);
        match resolve {
            Some(r) => autolink_code_spans(&html, r),
            None => html,
        }
    }
}

/// Re-render comrak's escaped fenced code blocks through the syntax
/// highlighter. Unknown languages pass through untouched.
fn highlight_fenced_blocks(html: &str) -> String {
    const OPEN: &str = "<pre><code class=\"language-";
    const CLOSE: &str = "</code></pre>";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let Some(p) = rest.find(OPEN) else {
            out.push_str(rest);
            return out;
        };
        let lang_start = p + OPEN.len();
        let Some(q) = rest[lang_start..].find("\">") else {
            out.push_str(rest);
            return out;
        };
        let lang = rest[lang_start..lang_start + q].to_owned();
        let content_start = lang_start + q + 2;
        let Some(endp) = rest[content_start..].find(CLOSE) else {
            out.push_str(rest);
            return out;
        };
        let inner = &rest[content_start..content_start + endp];
        out.push_str(&rest[..content_start]);
        match crate::render::highlight::block(&unescape(inner), &lang) {
            Some(hl) => out.push_str(&hl),
            None => out.push_str(inner),
        }
        out.push_str(CLOSE);
        rest = &rest[content_start + endp + CLOSE.len()..];
    }
}

/// Reverse comrak's HTML escaping (only these five entities appear in
/// its escaped code content). `&amp;` last so it cannot double-decode.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Rewrite `<code>Name</code>` spans that resolve to API items into
/// links, operating on rendered HTML. Fenced code blocks are untouched:
/// everything between `<pre` and `</pre>` passes through verbatim.
fn autolink_code_spans(html: &str, resolve: Resolver) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        // Copy any <pre> block verbatim if it comes before the next <code>.
        let next_code = rest.find("<code>");
        let next_pre = rest.find("<pre");
        match (next_code, next_pre) {
            (None, _) => {
                out.push_str(rest);
                return out;
            }
            (Some(c), Some(p)) if p < c => {
                let end = rest[p..]
                    .find("</pre>")
                    .map(|e| p + e + "</pre>".len())
                    .unwrap_or(rest.len());
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            }
            (Some(c), _) => {
                out.push_str(&rest[..c]);
                let after = &rest[c + "<code>".len()..];
                let Some(close) = after.find("</code>") else {
                    out.push_str(&rest[c..]);
                    return out;
                };
                let inner = &after[..close];
                match linkable_name(inner).and_then(|n| resolve(&n)) {
                    Some(href) => {
                        out.push_str(&format!(
                            "<a class=\"apilink\" href=\"{href}\"><code>{inner}</code></a>"
                        ));
                    }
                    None => {
                        out.push_str("<code>");
                        out.push_str(inner);
                        out.push_str("</code>");
                    }
                }
                rest = &after[close + "</code>".len()..];
            }
        }
    }
}

/// A code span is a link candidate if it looks like one identifier-ish
/// mention (no spaces, no operators beyond `:: . () ~`), decoded from
/// the few entities comrak may have escaped.
fn linkable_name(inner: &str) -> Option<String> {
    let decoded = inner.replace("&amp;", "&");
    if decoded.len() > 100
        || decoded.is_empty()
        || !decoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '(' | ')' | '~'))
    {
        return None;
    }
    Some(decoded)
}
