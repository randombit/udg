//! Shared doc-comment parser: a Doxygen-compatible tag layer whose free
//! text is treated as Markdown.
//!
//! This parses the working subset of tags that real codebases use
//! (`@param`, `@return`, `@throws`, `@warning`, `@note`, `@see`,
//! `@since`, `@deprecated`, `@brief`), not Doxygen's full command set.
//! Both `@tag` and `\tag` spellings are accepted. Unknown tags are left
//! in the body verbatim rather than dropped.

use crate::ir::{DocBlock, DocParam};

/// Parse a raw comment as returned by libclang (including the `/** */`
/// or `///` markers) or a Python docstring (pass the string contents).
pub fn parse(raw: &str) -> DocBlock {
    let text = strip_comment_markers(raw);
    parse_cleaned(&text)
}

/// Remove comment syntax: `/** */` fences and leading `*` gutters, or
/// `///` / `//!` prefixes. A `<` right after the opener (`/**<`, `///<`:
/// Doxygen's "documents the preceding declaration" marker) is syntax
/// too. Text without comment markers passes through.
fn strip_comment_markers(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("/*") {
        let mut inner = trimmed;
        for open in ["/**", "/*!", "/*"] {
            if let Some(rest) = inner.strip_prefix(open) {
                inner = rest.strip_prefix('<').unwrap_or(rest);
                break;
            }
        }
        inner = inner.strip_suffix("*/").unwrap_or(inner);
        inner
            .lines()
            .map(|line| {
                let l = line.trim_start();
                match l.strip_prefix('*') {
                    Some(rest) => rest.strip_prefix(' ').unwrap_or(rest),
                    None => l,
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else if trimmed.starts_with("///") || trimmed.starts_with("//!") {
        trimmed
            .lines()
            .map(|line| {
                let l = line.trim_start();
                let l = l
                    .strip_prefix("///")
                    .or_else(|| l.strip_prefix("//!"))
                    .or_else(|| l.strip_prefix("//"))
                    .unwrap_or(l);
                let l = l.strip_prefix('<').unwrap_or(l);
                l.strip_prefix(' ').unwrap_or(l)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        trimmed.to_owned()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tag {
    Brief,
    Param(String),
    Returns,
    Throws,
    Warning,
    Note,
    See,
    Since,
    Deprecated,
}

/// Sphinx-style field list line (`:param x: desc`, `:returns: desc`,
/// `:raises TypeError: desc`).
fn parse_field_line(line: &str) -> Option<(Tag, String)> {
    let l = line.trim_start();
    let rest = l.strip_prefix(':')?;
    let close = rest.find(':')?;
    let field = &rest[..close];
    let content = rest[close + 1..].trim_start().to_owned();
    let mut words = field.split_whitespace();
    let keyword = words.next()?;
    // Sphinx allows a type between keyword and name (`:param int x:`);
    // the parameter name is the LAST word.
    let args: Vec<&str> = words.collect();
    let arg = args.last().copied();
    match (keyword, arg) {
        ("param" | "parameter" | "arg" | "argument", Some(name)) => {
            Some((Tag::Param(name.to_owned()), content))
        }
        ("return" | "returns" | "rtype", None) => Some((Tag::Returns, content)),
        ("raise" | "raises" | "except" | "exception", ty) => Some((
            Tag::Throws,
            match ty {
                Some(t) => format!("{t}: {content}"),
                None => content,
            },
        )),
        _ => None,
    }
}

/// If `line` begins a tag (`@word`, `\word`, or a Sphinx field), return
/// the tag and the rest of the line.
fn parse_tag_line(line: &str) -> Option<(Tag, String)> {
    let l = line.trim_start();
    if l.starts_with(':') {
        return parse_field_line(l);
    }
    let rest = l.strip_prefix('@').or_else(|| l.strip_prefix('\\'))?;
    let word_end = rest
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(rest.len());
    let (word, mut after) = rest.split_at(word_end);
    after = after.trim_start();

    let tag = match word {
        "brief" | "short" => Tag::Brief,
        "param" | "arg" => {
            // Allow `@param[in] name desc` / `@param[out]` / `@param[in,out]`.
            if after.starts_with('[') {
                let close = after.find(']')?;
                after = after[close + 1..].trim_start();
            }
            let name_end = after.find(char::is_whitespace).unwrap_or(after.len());
            let (name, desc) = after.split_at(name_end);
            if name.is_empty() {
                return None;
            }
            return Some((Tag::Param(name.to_owned()), desc.trim_start().to_owned()));
        }
        "return" | "returns" | "result" | "retval" => Tag::Returns,
        "throw" | "throws" | "exception" => Tag::Throws,
        "warning" => Tag::Warning,
        "note" | "remark" | "remarks" => Tag::Note,
        "see" | "sa" => Tag::See,
        "since" => Tag::Since,
        "deprecated" => Tag::Deprecated,
        _ => return None,
    };
    Some((tag, after.to_owned()))
}

fn parse_cleaned(text: &str) -> DocBlock {
    let mut doc = DocBlock::default();
    let mut body_lines: Vec<String> = Vec::new();
    let mut brief: Option<String> = None;

    // (tag, accumulated content). Tag content continues until the next
    // tag or a blank line; text after a blank line returns to the body.
    let mut current: Option<(Tag, String)> = None;

    let flush = |doc: &mut DocBlock, brief: &mut Option<String>, cur: Option<(Tag, String)>| {
        let Some((tag, content)) = cur else { return };
        let content = content.trim().to_owned();
        match tag {
            Tag::Brief => *brief = Some(content),
            Tag::Param(name) => doc.params.push(DocParam {
                name,
                text: content,
            }),
            Tag::Returns => match &mut doc.returns {
                Some(r) => {
                    r.push_str("; ");
                    r.push_str(&content);
                }
                None => doc.returns = Some(content),
            },
            Tag::Throws => doc.throws.push(content),
            Tag::Warning => doc.warnings.push(content),
            Tag::Note => doc.notes.push(content),
            Tag::See => doc.see_also.push(content),
            Tag::Since => doc.since = Some(content),
            Tag::Deprecated => doc.deprecated = Some(content),
        }
    };

    for line in text.lines() {
        if let Some((tag, rest)) = parse_tag_line(line) {
            flush(&mut doc, &mut brief, current.take());
            current = Some((tag, rest));
        } else if line.trim().is_empty() {
            if current.is_some() {
                flush(&mut doc, &mut brief, current.take());
            } else if !body_lines.is_empty() {
                body_lines.push(String::new());
            }
        } else if let Some((_, content)) = &mut current {
            if !content.is_empty() {
                content.push(' ');
            }
            content.push_str(line.trim());
        } else {
            body_lines.push(line.trim_end().to_owned());
        }
    }
    flush(&mut doc, &mut brief, current.take());

    while body_lines.last().is_some_and(|l| l.is_empty()) {
        body_lines.pop();
    }
    let body = body_lines.join("\n");
    let body = body.trim();

    doc.summary = brief.clone().or_else(|| {
        body.split("\n\n")
            .next()
            .map(|p| p.replace('\n', " ").trim().to_owned())
            .filter(|p| !p.is_empty())
    });
    // Body is the display text, so an explicit `@brief` must lead it —
    // otherwise a brief-only comment counts as documented yet renders
    // blank. (A derived summary is already the body's first paragraph.)
    let body = match brief.filter(|b| !b.is_empty()) {
        Some(b) if body.is_empty() => b,
        Some(b) => format!("{b}\n\n{body}"),
        None => body.to_owned(),
    };
    if !body.is_empty() {
        doc.body = Some(body);
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_comment_with_tags() {
        let doc = parse(
            "/**\n\
             * Frob the widget.\n\
             *\n\
             * Longer discussion of\n\
             * frobbing.\n\
             * @param amount how much to frob,\n\
             *        possibly on two lines\n\
             * @param fast whether to hurry\n\
             * @return the frob count\n\
             * @see Gadget\n\
             */",
        );
        assert_eq!(doc.summary.as_deref(), Some("Frob the widget."));
        assert_eq!(
            doc.body.as_deref(),
            Some("Frob the widget.\n\nLonger discussion of\nfrobbing.")
        );
        assert_eq!(doc.params.len(), 2);
        assert_eq!(doc.params[0].name, "amount");
        assert_eq!(
            doc.params[0].text,
            "how much to frob, possibly on two lines"
        );
        assert_eq!(doc.params[1].name, "fast");
        assert_eq!(doc.returns.as_deref(), Some("the frob count"));
        assert_eq!(doc.see_also, vec!["Gadget"]);
    }

    #[test]
    fn multiline_summary_is_one_paragraph() {
        let doc = parse(
            "/**\n\
             * Return the estimated strength of the underlying key against\n\
             * the best currently known attack.\n\
             */",
        );
        assert_eq!(
            doc.summary.as_deref(),
            Some(
                "Return the estimated strength of the underlying key against the best currently known attack."
            )
        );
    }

    #[test]
    fn triple_slash_and_backslash_tags() {
        let doc = parse("/// Does a thing.\n/// \\param x the input\n/// \\returns nothing");
        assert_eq!(doc.summary.as_deref(), Some("Does a thing."));
        assert_eq!(doc.params[0].name, "x");
        assert_eq!(doc.returns.as_deref(), Some("nothing"));
    }

    #[test]
    fn brief_overrides_summary_and_param_in_out() {
        let doc = parse(
            "/**\n\
             * @brief Short form.\n\
             *\n\
             * Some body text.\n\
             * @param[out] result where output goes\n\
             */",
        );
        assert_eq!(doc.summary.as_deref(), Some("Short form."));
        // The brief leads the body: body is what pages display.
        assert_eq!(doc.body.as_deref(), Some("Short form.\n\nSome body text."));
        assert_eq!(doc.params[0].name, "result");
        assert_eq!(doc.params[0].text, "where output goes");

        // A brief-only comment still has a rendering body.
        let doc = parse("/** @brief Lone brief. */");
        assert_eq!(doc.summary.as_deref(), Some("Lone brief."));
        assert_eq!(doc.body.as_deref(), Some("Lone brief."));
    }

    #[test]
    fn unknown_tags_and_emails_stay_in_body() {
        let doc = parse("/**\n* Contact lloyd@randombit.net about this.\n* @custom stays put\n*/");
        assert!(doc.body.as_deref().unwrap().contains("lloyd@randombit.net"));
        assert!(doc.body.as_deref().unwrap().contains("@custom stays put"));
    }

    #[test]
    fn plain_docstring_passthrough() {
        let doc = parse("Returns the major number of the library version.");
        assert_eq!(
            doc.summary.as_deref(),
            Some("Returns the major number of the library version.")
        );
        assert!(doc.params.is_empty());
    }

    #[test]
    fn typed_param_fields_take_the_last_word_as_name() {
        let doc = parse(":param int value: the size\n:param value2: plain");
        assert_eq!(doc.params[0].name, "value");
        assert_eq!(doc.params[0].text, "the size");
        assert_eq!(doc.params[1].name, "value2");
    }

    #[test]
    fn sphinx_field_lists() {
        let doc = parse(
            "Frob the widget.\n\n\
             :param amount: how much to frob\n\
             :returns: the frob count\n\
             :raises ValueError: when amount is negative\n",
        );
        assert_eq!(doc.summary.as_deref(), Some("Frob the widget."));
        assert_eq!(doc.params.len(), 1);
        assert_eq!(doc.params[0].name, "amount");
        assert_eq!(doc.returns.as_deref(), Some("the frob count"));
        assert_eq!(doc.throws, vec!["ValueError: when amount is negative"]);
        // Bare `::`-ish lines must not be eaten as fields.
        let doc2 = parse("Uses Botan::HashFunction.\n:unknown thing: stays\n");
        assert!(
            doc2.body
                .as_deref()
                .unwrap()
                .contains(":unknown thing: stays")
        );
    }

    #[test]
    fn warning_note_since_deprecated() {
        let doc = parse(
            "/**\n\
             * A thing.\n\
             * @warning sharp edges\n\
             * @note handle gently\n\
             * @since 3.2\n\
             * @deprecated use OtherThing instead\n\
             */",
        );
        assert_eq!(doc.warnings, vec!["sharp edges"]);
        assert_eq!(doc.notes, vec!["handle gently"]);
        assert_eq!(doc.since.as_deref(), Some("3.2"));
        assert_eq!(doc.deprecated.as_deref(), Some("use OtherThing instead"));
    }
}
