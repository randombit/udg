//! Documentation lints: the checks that make docs a CI gate rather than
//! an aspiration. Everything here reports file:line findings suitable
//! for failing a build.

use crate::ir::{Access, Item, ItemKind};
use crate::link::{Resolution, SymbolIndex};

pub struct Finding {
    pub lint: &'static str,
    pub file: String,
    pub line: u32,
    pub message: String,
}

pub struct TreeStats {
    pub prefix: String,
    pub documentable: usize,
    pub documented: usize,
}

impl TreeStats {
    pub fn percent(&self) -> f64 {
        if self.documentable == 0 {
            100.0
        } else {
            100.0 * self.documented as f64 / self.documentable as f64
        }
    }
}

/// What `check_tree` enforces beyond the always-on lints.
#[derive(Default, Clone, Copy)]
pub struct Strictness {
    /// Flag every public item lacking documentation.
    pub require_docs: bool,
    /// Flag documented functions that document none of their
    /// parameters. (The always-on lint only fires once at least one
    /// param is documented; this closes the none-at-all gap.)
    pub require_param_docs: bool,
}

/// Run all lints over one tree, appending findings and returning
/// coverage statistics.
pub fn check_tree(
    prefix: &str,
    items: &[Item],
    symbols: &SymbolIndex,
    strict: Strictness,
    findings: &mut Vec<Finding>,
) -> TreeStats {
    let mut stats = TreeStats {
        prefix: prefix.to_owned(),
        documentable: 0,
        documented: 0,
    };
    for item in items {
        walk(item, symbols, strict, findings, &mut stats);
    }
    stats
}

fn loc(item: &Item) -> (String, u32) {
    item.source
        .as_ref()
        .map(|s| (s.file.clone(), s.line_start))
        .unwrap_or_else(|| ("<unknown>".into(), 0))
}

fn walk(
    item: &Item,
    symbols: &SymbolIndex,
    strict: Strictness,
    findings: &mut Vec<Finding>,
    stats: &mut TreeStats,
) {
    if item.access == Some(Access::Private) {
        return;
    }
    // Destructors are exempt: documenting them is not a convention
    // anywhere, and "zero undocumented" should equal 100% coverage.
    if !matches!(
        item.kind,
        ItemKind::Namespace | ItemKind::Enumerator | ItemKind::Destructor
    ) {
        stats.documentable += 1;
        if item.docs.as_ref().is_some_and(|d| d.summary.is_some()) {
            stats.documented += 1;
        } else if strict.require_docs {
            let (file, line) = loc(item);
            findings.push(Finding {
                lint: "undocumented",
                file,
                line,
                message: format!("`{}` has no documentation", item.qualified_name),
            });
        }
    }

    // Inherited docs describe the base declaration's signature; its
    // parameter names may legitimately differ from this override's.
    if let (Some(sig), Some(docs), None) = (&item.signature, &item.docs, &item.docs_from) {
        // Parameter names as declared, ignoring Python's marker
        // pseudo-params and leading stars.
        let sig_names: Vec<String> = sig
            .params
            .iter()
            .filter_map(|p| p.name.as_deref())
            .map(|n| n.trim_start_matches('*').to_owned())
            .filter(|n| !n.is_empty() && n != "/")
            .collect();

        let (file, line) = loc(item);
        for dp in &docs.params {
            if !sig_names.iter().any(|n| n == &dp.name) {
                findings.push(Finding {
                    lint: "param-mismatch",
                    file: file.clone(),
                    line,
                    message: format!(
                        "@param `{}` does not match any parameter of `{}`",
                        dp.name, item.qualified_name
                    ),
                });
            }
        }
        // By default missing @param docs only count once the comment
        // documents at least one parameter (fully undocumented items
        // are coverage's concern); require_param_docs closes that gap
        // and demands every parameter of a documented function.
        if !docs.params.is_empty() || strict.require_param_docs {
            for n in &sig_names {
                if !docs.params.iter().any(|dp| &dp.name == n) {
                    findings.push(Finding {
                        lint: "param-undocumented",
                        file: file.clone(),
                        line,
                        message: format!(
                            "parameter `{n}` of `{}` is not documented",
                            item.qualified_name
                        ),
                    });
                }
            }
        }
    }

    // Explicit references (@see) must resolve.
    if let Some(docs) = &item.docs {
        let scope = parent_scope(&item.qualified_name);
        let (file, line) = loc(item);
        for s in &docs.see_also {
            // Prose after the name is common ("Foo for details"): check
            // only single-token references.
            let token = s.trim();
            if token.contains(char::is_whitespace) || token.contains("://") {
                continue;
            }
            match symbols.resolve(token, scope, Some(item.lang)) {
                Resolution::Unique(_) => {}
                Resolution::Ambiguous => findings.push(Finding {
                    lint: "ambiguous-ref",
                    file: file.clone(),
                    line,
                    message: format!("@see `{token}` in `{}` is ambiguous", item.qualified_name),
                }),
                Resolution::Unknown => findings.push(Finding {
                    lint: "broken-ref",
                    file: file.clone(),
                    line,
                    message: format!(
                        "@see `{token}` in `{}` does not resolve",
                        item.qualified_name
                    ),
                }),
            }
        }
    }

    for c in &item.children {
        walk(c, symbols, strict, findings, stats);
    }
}

/// Parse a hand-written signature directive argument into the declared
/// name and argument count. `X509_DN subject_dn() const` -> ("subject_dn", 0).
pub fn parse_handwritten_sig(sig: &str) -> Option<(String, usize)> {
    let open = sig.find('(')?;
    let prefix = sig[..open].trim_end();
    let name = if let Some(pos) = prefix.rfind("operator") {
        // Normalize `operator !` / `operator ==` spellings.
        prefix[pos..].replace(' ', "")
    } else {
        let chars: Vec<char> = prefix.chars().collect();
        let mut i = chars.len();
        while i > 0
            && (chars[i - 1].is_ascii_alphanumeric() || matches!(chars[i - 1], '_' | ':' | '~'))
        {
            i -= 1;
        }
        chars[i..].iter().collect()
    };
    if name.is_empty() {
        return None;
    }
    let mut depth = 1usize;
    let mut count = 0usize;
    let mut has_content = false;
    for ch in sig[open + 1..].chars() {
        match ch {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => count += 1,
            c if !c.is_whitespace() => has_content = true,
            _ => {}
        }
    }
    Some((name, if has_content { count + 1 } else { 0 }))
}

/// Does the hand-written argument count fit any overload among the
/// candidates? Constructors are matched against a class candidate's
/// Constructor children. Returns (any_signature_seen, matched).
pub fn sig_matches_any(count: usize, candidates: &[&Item]) -> (bool, bool) {
    let mut any = false;
    for c in candidates {
        let sigs: Vec<&crate::ir::Signature> = match c.kind {
            ItemKind::Class | ItemKind::Struct => c
                .children
                .iter()
                .filter(|ch| ch.kind == ItemKind::Constructor)
                .filter_map(|ch| ch.signature.as_ref())
                .collect(),
            _ => c.signature.as_ref().into_iter().collect(),
        };
        for s in sigs {
            any = true;
            let real: Vec<_> = s
                .params
                .iter()
                .filter(|p| !matches!(p.name.as_deref(), Some("*") | Some("/")))
                .collect();
            let required = real.iter().filter(|p| p.default.is_none()).count();
            if count >= required && count <= real.len() {
                return (true, true);
            }
        }
    }
    (any, false)
}

/// The scope references resolve in: the containing item.
fn parent_scope(qualified: &str) -> Option<&str> {
    qualified
        .rsplit_once("::")
        .or_else(|| qualified.rsplit_once('.'))
        .map(|(parent, _)| parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Language;

    #[test]
    fn require_docs_flags_only_when_enabled() {
        let mut ns = Item::new(Language::Cpp, ItemKind::Namespace, "N", "N");
        let mut documented = Item::new(Language::Cpp, ItemKind::Class, "A", "N::A");
        documented.docs = Some(crate::ir::DocBlock {
            summary: Some("docs".into()),
            ..crate::ir::DocBlock::default()
        });
        let bare = Item::new(Language::Cpp, ItemKind::Class, "B", "N::B");
        let dtor = Item::new(Language::Cpp, ItemKind::Destructor, "~B", "N::B::~B");
        ns.children = vec![documented, bare, dtor];
        let items = vec![ns];
        let symbols = SymbolIndex::build(std::iter::once((items.as_slice(), None)));

        let mut findings = Vec::new();
        let stats = check_tree(
            "cpp",
            &items,
            &symbols,
            Strictness::default(),
            &mut findings,
        );
        assert_eq!(findings.len(), 0);
        assert_eq!((stats.documentable, stats.documented), (2, 1));

        let mut findings = Vec::new();
        check_tree(
            "cpp",
            &items,
            &symbols,
            Strictness {
                require_docs: true,
                ..Strictness::default()
            },
            &mut findings,
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].lint, "undocumented");
        assert!(findings[0].message.contains("N::B"));
    }

    #[test]
    fn require_param_docs_flags_wholly_undocumented_params() {
        let mut ns = Item::new(Language::Cpp, ItemKind::Namespace, "N", "N");
        let mut f = Item::new(Language::Cpp, ItemKind::Function, "f", "N::f");
        f.signature = Some(crate::ir::Signature {
            params: vec![
                crate::ir::Param {
                    name: Some("a".into()),
                    ty: "int".into(),
                    default: None,
                },
                crate::ir::Param {
                    name: Some("b".into()),
                    ty: "int".into(),
                    default: None,
                },
            ],
            ..crate::ir::Signature::default()
        });
        // Documented, but with zero @param entries.
        f.docs = Some(crate::ir::DocBlock {
            summary: Some("Does f things.".into()),
            ..crate::ir::DocBlock::default()
        });
        ns.children.push(f);
        let items = vec![ns];
        let symbols = SymbolIndex::build(std::iter::once((items.as_slice(), None)));

        // Lax: coverage's concern, no findings.
        let mut findings = Vec::new();
        check_tree(
            "cpp",
            &items,
            &symbols,
            Strictness::default(),
            &mut findings,
        );
        assert_eq!(findings.len(), 0);

        // Strict: one finding per undocumented parameter.
        let mut findings = Vec::new();
        check_tree(
            "cpp",
            &items,
            &symbols,
            Strictness {
                require_param_docs: true,
                ..Strictness::default()
            },
            &mut findings,
        );
        assert_eq!(
            findings.len(),
            2,
            "{:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
        assert!(findings.iter().all(|f| f.lint == "param-undocumented"));
    }

    #[test]
    fn inherited_docs_count_and_skip_param_lints() {
        let mut ns = Item::new(Language::Cpp, ItemKind::Namespace, "N", "N");
        let mut m = Item::new(Language::Cpp, ItemKind::Method, "f", "N::D::f");
        m.signature = Some(crate::ir::Signature {
            params: vec![crate::ir::Param {
                name: Some("renamed".into()),
                ty: "int".into(),
                default: None,
            }],
            ..crate::ir::Signature::default()
        });
        // Inherited from a base whose parameter was named differently:
        // documented for coverage, exempt from param lints.
        m.docs = Some(crate::ir::DocBlock {
            summary: Some("Base docs.".into()),
            params: vec![crate::ir::DocParam {
                name: "orig".into(),
                text: "the input".into(),
            }],
            ..crate::ir::DocBlock::default()
        });
        m.docs_from = Some("B::f".into());
        ns.children.push(m);
        let items = vec![ns];
        let symbols = SymbolIndex::build(std::iter::once((items.as_slice(), None)));

        let mut findings = Vec::new();
        let stats = check_tree(
            "cpp",
            &items,
            &symbols,
            Strictness {
                require_docs: true,
                ..Strictness::default()
            },
            &mut findings,
        );
        assert_eq!(
            findings.len(),
            0,
            "{:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
        assert_eq!((stats.documentable, stats.documented), (1, 1));
    }
}
