//! The udg document IR: the language-neutral model that all frontends emit
//! and all later stages (linker, renderer, checks) consume.
//!
//! Everything here is serde-serializable; `udg extract --format json` dumps
//! this model directly, and its JSON shape is intended to become a stable,
//! documented interface (like rustdoc's JSON output).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Cpp,
    C,
    Python,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Namespace,
    Class,
    Struct,
    Enum,
    Enumerator,
    Function,
    Method,
    Constructor,
    Destructor,
    ConversionFunction,
    TypeAlias,
    Variable,
    Field,
    Concept,
    Macro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Param {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Signature {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub params: Vec<Param>,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_const: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_virtual: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_pure_virtual: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_override: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_final: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_static: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_noexcept: bool,
    /// Template parameter list as written, e.g. `template<typename T>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_params: Option<String>,
}

/// A parsed documentation comment: Markdown body plus the structured
/// fields shared by every comment dialect udg accepts (Doxygen tags,
/// Sphinx-style and Google-style docstring sections).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DocBlock {
    /// First paragraph (or explicit `@brief`), for item listings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Full Markdown body, including the summary paragraph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub params: Vec<DocParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub throws: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub see_also: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocParam {
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpan {
    pub file: String,
    pub line_start: u32,
    pub line_end: u32,
}

/// One entry of a project's module map — how items group for navigation.
/// Module structure is project knowledge supplied via config (a static
/// file or a command emitting `{"modules": [...]}`), never inferred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    /// Stable id; may contain `/` for nesting (e.g. `hash/sha2_32`).
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub brief: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent: Option<String>,
    /// Source files whose items belong to this module.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub headers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// Stable path-derived id, e.g. `cpp:Botan::Public_Key::check_key`.
    /// Overloads currently share an id; disambiguation is an M1 concern.
    pub id: String,
    pub lang: Language,
    pub kind: ItemKind,
    pub name: String,
    pub qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<DocBlock>,
    /// When `docs` were inherited from an overridden base declaration
    /// (Doxygen INHERIT_DOCS semantics), the base's `Class::method`
    /// name. Lints that compare docs against this item's own signature
    /// (e.g. parameter names) should stand down.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub docs_from: Option<String>,
    /// Version the item was introduced (e.g. from BOTAN_PUBLIC_API(2,0)
    /// via attribute mapping, or a doc `@since` tag).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Deprecation message, from mapped attributes, `[[deprecated]]`, or
    /// a doc `@deprecated` tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    /// Public base classes (classes/structs only).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub bases: Vec<String>,
    /// Enum underlying type, alias target, or variable/field type.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    /// Enumerator value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSpan>,
    /// Module id from the project's module map. Namespaces span modules
    /// and carry none; other items inherit their lexical parent's.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub module: Option<String>,
    /// Cross-language binding edges: ids of corresponding items in other
    /// language trees, filled by the linker from the binding map.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub bindings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<Item>,
}

impl Item {
    pub fn new(lang: Language, kind: ItemKind, name: &str, qualified_name: &str) -> Self {
        let prefix = match lang {
            Language::Cpp => "cpp",
            Language::C => "c",
            Language::Python => "py",
        };
        Item {
            id: format!("{prefix}:{qualified_name}"),
            lang,
            kind,
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature: None,
            docs: None,
            docs_from: None,
            since: None,
            deprecated: None,
            access: None,
            bases: Vec::new(),
            ty: None,
            value: None,
            source: None,
            module: None,
            bindings: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Mutable depth-first traversal.
    pub fn walk_mut(&mut self, f: &mut impl FnMut(&mut Item)) {
        f(self);
        for c in &mut self.children {
            c.walk_mut(f);
        }
    }

    /// Depth-first traversal over this item and all descendants.
    pub fn walk(&self, f: &mut impl FnMut(&Item)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }
}

fn is_false(v: &bool) -> bool {
    !v
}
