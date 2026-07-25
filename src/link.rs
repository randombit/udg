//! The linker: symbol resolution for autolinks, and cross-language
//! binding edges from the project's binding map.

use std::collections::HashMap;

use crate::ir::{Item, ItemKind, Language};
use anyhow::{Context, Result};
use serde::Deserialize;

/// Name → item-id index over every tree, used to resolve `` `Name` ``
/// mentions in doc text and `@see` references.
pub struct SymbolIndex {
    /// Qualified name (both full and root-namespace-stripped forms) → ids.
    by_qualified: HashMap<String, Vec<String>>,
    /// Last name component → ids.
    by_name: HashMap<String, Vec<String>>,
    /// Every `::`/`.` scope suffix of every qualified name. Prose
    /// qualifies references at arbitrary depth (`AttributeType::Label`
    /// for `Botan::PKCS11::AttributeType::Label`); this answers "does
    /// the referenced symbol exist" without linking it.
    suffixes: std::collections::HashSet<String>,
}

pub enum Resolution {
    Unique(String),
    Ambiguous,
    Unknown,
}

impl SymbolIndex {
    pub fn build<'a>(trees: impl Iterator<Item = (&'a [Item], Option<&'a str>)>) -> SymbolIndex {
        let mut index = SymbolIndex {
            by_qualified: HashMap::new(),
            by_name: HashMap::new(),
            suffixes: std::collections::HashSet::new(),
        };
        for (items, root_ns) in trees {
            for item in items {
                item.walk(&mut |i| {
                    if matches!(i.kind, ItemKind::Namespace) {
                        return;
                    }
                    index
                        .by_qualified
                        .entry(i.qualified_name.clone())
                        .or_default()
                        .push(i.id.clone());
                    if let Some(ns) = root_ns
                        && let Some(short) = i
                            .qualified_name
                            .strip_prefix(ns)
                            .and_then(|r| r.strip_prefix("::"))
                    {
                        index
                            .by_qualified
                            .entry(short.to_owned())
                            .or_default()
                            .push(i.id.clone());
                    }
                    index
                        .by_name
                        .entry(i.name.clone())
                        .or_default()
                        .push(i.id.clone());
                    let sep = if i.qualified_name.contains("::") {
                        "::"
                    } else {
                        "."
                    };
                    let segments: Vec<&str> = i.qualified_name.split(sep).collect();
                    for start in 0..segments.len() {
                        index.suffixes.insert(segments[start..].join(sep));
                    }
                });
            }
        }
        // Overloads share an id and each pushed a copy; without dedup a
        // single overloaded function reads as ambiguous and silently
        // stops resolving everywhere.
        for ids in index.by_qualified.values_mut() {
            ids.sort();
            ids.dedup();
        }
        for ids in index.by_name.values_mut() {
            ids.sort();
            ids.dedup();
        }
        index
    }

    /// Resolve a doc-text mention. `scope` is the qualified name of the
    /// item whose docs are being rendered (members resolve first);
    /// `lang` prefers same-language candidates, so a C++ signature's
    /// `RandomNumberGenerator` wins over Python's class of the same name.
    /// Register presence-only names (external-header declarations,
    /// macros): `exists` acknowledges them; `resolve` never links them.
    pub fn add_presence(&mut self, names: impl IntoIterator<Item = String>) {
        self.suffixes.extend(names);
    }

    /// Does any item answer to this name at any qualification depth?
    /// (Presence only — use `resolve` to link.)
    pub fn exists(&self, raw: &str) -> bool {
        let name = raw.trim().trim_end_matches("()");
        self.suffixes.contains(name) || self.by_name.contains_key(name)
    }

    pub fn resolve(&self, raw: &str, scope: Option<&str>, lang: Option<Language>) -> Resolution {
        let pick = |ids: &[String]| -> Resolution {
            if let [id] = ids {
                return Resolution::Unique(id.clone());
            }
            if let Some(lang) = lang {
                let prefix = match lang {
                    Language::Cpp => "cpp:",
                    Language::C => "c:",
                    Language::Python => "py:",
                };
                let same: Vec<&String> = ids.iter().filter(|i| i.starts_with(prefix)).collect();
                if let [id] = same.as_slice() {
                    return Resolution::Unique((*id).clone());
                }
            }
            Resolution::Ambiguous
        };
        let name = raw.trim().trim_end_matches("()");
        if name.is_empty()
            || !name
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            return Resolution::Unknown;
        }
        // Scoped member lookup: `sign` inside PK_Signer's docs.
        if let Some(scope) = scope {
            for sep in ["::", "."] {
                if let Some(ids) = self.by_qualified.get(&format!("{scope}{sep}{name}"))
                    && let [id] = ids.as_slice()
                {
                    return Resolution::Unique(id.clone());
                }
            }
        }
        if let Some(ids) = self.by_qualified.get(name) {
            return pick(ids);
        }
        if let Some(ids) = self.by_name.get(name) {
            return pick(ids);
        }
        Resolution::Unknown
    }
}

/// C++ allows re-declaring a type alias in several headers (Botan's
/// `SymmetricKey` appears in symkey.h and twice in TLS headers). Keep
/// only the first declaration of identical namespace-scope aliases:
/// later copies would make the name ambiguous (so autolinks refuse to
/// resolve it) and produce duplicate pages and search entries. Aliases
/// sharing a name but targeting different types are both kept — that
/// ambiguity is real.
pub fn dedupe_alias_redeclarations(items: &mut Vec<Item>) {
    fn walk(items: &mut Vec<Item>, seen: &mut std::collections::HashSet<(String, Option<String>)>) {
        items.retain(|i| {
            i.kind != ItemKind::TypeAlias || seen.insert((i.qualified_name.clone(), i.ty.clone()))
        });
        for i in items.iter_mut() {
            if i.kind == ItemKind::Namespace {
                walk(&mut i.children, seen);
            }
        }
    }
    walk(items, &mut std::collections::HashSet::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Language;

    #[test]
    fn alias_redeclarations_collapse() {
        let alias = |ty: &str| {
            let mut i = Item::new(Language::Cpp, ItemKind::TypeAlias, "K", "N::K");
            i.ty = Some(ty.into());
            i
        };
        let mut ns = Item::new(Language::Cpp, ItemKind::Namespace, "N", "N");
        ns.children = vec![alias("O"), alias("O"), alias("Other")];
        let mut items = vec![ns];
        dedupe_alias_redeclarations(&mut items);
        // Identical redeclaration dropped; different-target alias kept.
        assert_eq!(items[0].children.len(), 2);
    }

    #[test]
    fn overloads_share_an_id_and_resolve_uniquely() {
        let mut ns = Item::new(Language::Cpp, ItemKind::Namespace, "N", "N");
        let mut class = Item::new(Language::Cpp, ItemKind::Class, "C", "N::C");
        // Two overloads: same name, same id.
        class.children.push(Item::new(
            Language::Cpp,
            ItemKind::Method,
            "update",
            "N::C::update",
        ));
        class.children.push(Item::new(
            Language::Cpp,
            ItemKind::Method,
            "update",
            "N::C::update",
        ));
        ns.children.push(class);
        let items = vec![ns];
        let symbols = SymbolIndex::build(std::iter::once((items.as_slice(), None)));
        assert!(matches!(
            symbols.resolve("N::C::update", None, None),
            Resolution::Unique(_)
        ));
        assert!(matches!(
            symbols.resolve("update", Some("N::C"), None),
            Resolution::Unique(_)
        ));
    }
}

/// One entry of the binding map file: a correspondence between API
/// surfaces. `c_prefix` matches a whole family of C functions.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindEntry {
    #[serde(default)]
    pub c_prefix: Option<String>,
    #[serde(default)]
    pub cpp: Option<String>,
    #[serde(default)]
    pub py: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BindFile {
    #[serde(default)]
    bind: Vec<BindEntry>,
}

/// A family edge: a class ↔ a whole `botan_x_*` C function family
/// (rendered as a prefilled-search link, not N individual links).
pub struct Family {
    pub owner_id: String,
    pub prefix: String,
    pub count: usize,
}

pub struct LinkOutput {
    pub families: Vec<Family>,
    pub warnings: Vec<String>,
}

/// Apply the binding map: fill `Item.bindings` with direct cross-language
/// ids, and produce family edges for C prefix groups.
pub fn apply_bindings(
    map_toml: &str,
    trees: &mut [(&mut Vec<Item>, Option<&str>)],
) -> Result<LinkOutput> {
    let file: BindFile = toml::from_str(map_toml).context("invalid binding map")?;
    let mut out = LinkOutput {
        families: Vec::new(),
        warnings: Vec::new(),
    };

    // Index qualified name -> id per language, and collect C item names.
    let mut lookup: HashMap<(Language, String), String> = HashMap::new();
    let mut c_functions: Vec<(String, String)> = Vec::new(); // (name, id)
    for (items, root_ns) in trees.iter() {
        for item in items.iter() {
            item.walk(&mut |i| {
                lookup.insert((i.lang, i.qualified_name.clone()), i.id.clone());
                if let Some(ns) = root_ns
                    && let Some(short) = i
                        .qualified_name
                        .strip_prefix(*ns)
                        .and_then(|r| r.strip_prefix("::"))
                {
                    lookup.insert((i.lang, short.to_owned()), i.id.clone());
                }
                if i.lang == Language::C {
                    c_functions.push((i.name.clone(), i.id.clone()));
                }
            });
        }
    }

    // Resolve each map entry to concrete ids.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for entry in &file.bind {
        let cpp_id = entry
            .cpp
            .as_ref()
            .and_then(|n| lookup.get(&(Language::Cpp, n.clone())).cloned());
        if entry.cpp.is_some() && cpp_id.is_none() {
            out.warnings.push(format!(
                "binding map: C++ item not found: {}",
                entry.cpp.as_deref().unwrap_or_default()
            ));
        }
        let py_id = entry
            .py
            .as_ref()
            .and_then(|n| lookup.get(&(Language::Python, n.clone())).cloned());
        if entry.py.is_some() && py_id.is_none() {
            out.warnings.push(format!(
                "binding map: Python item not found: {}",
                entry.py.as_deref().unwrap_or_default()
            ));
        }
        let direct: Vec<String> = [cpp_id.clone(), py_id.clone()]
            .into_iter()
            .flatten()
            .collect();

        // cpp <-> py direct edges.
        for id in &direct {
            for other in &direct {
                if other != id {
                    edges.entry(id.clone()).or_default().push(other.clone());
                }
            }
        }

        if let Some(prefix) = &entry.c_prefix {
            let members: Vec<&(String, String)> = c_functions
                .iter()
                .filter(|(name, _)| name.starts_with(prefix))
                .collect();
            if members.is_empty() {
                out.warnings
                    .push(format!("binding map: no C functions match prefix {prefix}"));
                continue;
            }
            // Each C function links to the class(es); the classes get one
            // family edge to the whole group.
            for (_, c_id) in &members {
                edges
                    .entry(c_id.clone())
                    .or_default()
                    .extend(direct.clone());
            }
            for id in &direct {
                out.families.push(Family {
                    owner_id: id.clone(),
                    prefix: prefix.clone(),
                    count: members.len(),
                });
            }
        }
    }

    for (items, _) in trees.iter_mut() {
        for item in items.iter_mut() {
            item.walk_mut(&mut |i| {
                if let Some(b) = edges.get(&i.id) {
                    let mut b = b.clone();
                    b.sort();
                    b.dedup();
                    i.bindings = b;
                }
            });
        }
    }
    Ok(out)
}
