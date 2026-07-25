//! Build-time syntax highlighting via syntect, emitting class-based
//! spans (`hl-` prefixed) themed by CSS variables — no highlight.js,
//! and colors track light/dark like everything else.

use std::sync::LazyLock;

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

fn find_syntax(token: &str) -> Option<&'static SyntaxReference> {
    let token = match token {
        // Common fence names, and headers-are-C++ policy for .h files.
        "cpp" | "c++" | "h" | "hpp" => "C++",
        "console" | "shell" | "sh" => "Shell-Unix-Generic",
        "py" => "Python",
        "text" | "" | "none" => return None,
        other => other,
    };
    SYNTAXES.find_syntax_by_token(token)
}

/// The scope classes the stylesheet actually colors (see the `.hl-*`
/// rules in static/style.css; themes recolor the same set via the
/// `--hl-*` variables). Everything else syntect emits is pruned.
const STYLED: &[&str] = &[
    "hl-comment",
    "hl-string",
    "hl-keyword",
    "hl-storage",
    "hl-constant",
    "hl-entity",
    "hl-support",
];

/// Highlight a code block; returns inner HTML for `<code>`, or None when
/// the language is unknown (caller falls back to plain escaping).
pub fn block(code: &str, lang: &str) -> Option<String> {
    let syntax = find_syntax(lang)?;
    let mut generator = ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAXES, CLASS_STYLE);
    for line in LinesWithEndings::from(code) {
        generator
            .parse_html_for_line_which_includes_newline(line)
            .ok()?;
    }
    Some(prune(&generator.finalize()))
}

/// Strip syntect's output down to what the stylesheet can see: spans
/// keep only STYLED classes, and spans left with none unwrap entirely.
/// Most tokens are punctuation/variable/meta scopes that carry no
/// styling — on real headers this halves the HTML again.
fn prune(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    // For each open span: whether its close tag should be emitted.
    let mut kept: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < html.len() {
        let rest = &html[i..];
        if let Some(after) = rest.strip_prefix("<span class=\"") {
            let Some(q) = after.find('"') else {
                out.push_str(rest);
                break;
            };
            let classes = &after[..q];
            let styled: Vec<&str> = classes.split(' ').filter(|c| STYLED.contains(c)).collect();
            if styled.is_empty() {
                kept.push(false);
            } else {
                kept.push(true);
                out.push_str("<span class=\"");
                out.push_str(&styled.join(" "));
                out.push_str("\">");
            }
            i += "<span class=\"".len() + q + "\">".len();
        } else if rest.starts_with("</span>") {
            if kept.pop().unwrap_or(true) {
                out.push_str("</span>");
            }
            i += "</span>".len();
        } else {
            let c = rest.chars().next().expect("in-bounds");
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_cpp() {
        let html = block("int x = 1; // note\n", "cpp").expect("cpp syntax");
        assert!(html.contains("hl-"), "{html}");
        assert!(html.contains("int"));
        assert!(html.contains("comment") || html.contains("hl-comment"));
    }

    #[test]
    fn unknown_language_is_none() {
        assert!(block("whatever", "nosuchlang").is_none());
        assert!(block("plain", "text").is_none());
    }

    #[test]
    fn block_flows_across_lines() {
        // Spans may cross newlines; the whole fragment stays balanced.
        let html = block("/* a\n   b */\nint x;\n", "cpp").expect("cpp");
        assert_eq!(
            html.matches("<span").count(),
            html.matches("</span>").count()
        );
        assert!(html.contains("hl-comment"));
    }

    #[test]
    fn prune_drops_unstyled_scopes() {
        let html = block("void f(int a) { return; }\n", "cpp").expect("cpp");
        // Balanced after unwrapping, styled scopes kept, noise gone.
        assert_eq!(
            html.matches("<span").count(),
            html.matches("</span>").count()
        );
        assert!(
            html.contains("hl-storage") || html.contains("hl-keyword"),
            "{html}"
        );
        for noise in ["hl-source", "hl-punctuation", "hl-meta", "hl-variable"] {
            assert!(!html.contains(noise), "unpruned `{noise}`: {html}");
        }
        // Text content is untouched.
        assert!(html.contains("return"));
    }
}
