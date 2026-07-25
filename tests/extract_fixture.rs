//! End-to-end extraction test over the self-contained fixture header.
//!
//! Single test function: libclang may only be initialized once per
//! process, so all assertions share one extraction.

use std::path::Path;

use udg::frontend::clang::{AttributeRule, extract_header};
use udg::ir::{Access, Item, ItemKind};

fn find<'a>(items: &'a [Item], name: &str) -> &'a Item {
    items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("no item named {name} in {:?}", names(items)))
}

fn names(items: &[Item]) -> Vec<&str> {
    items.iter().map(|i| i.name.as_str()).collect()
}

#[test]
fn extracts_fixture_header() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/widget.hpp");
    let rules = vec![
        AttributeRule {
            macro_name: "TEST_API".into(),
            arg_names: vec!["major".into(), "minor".into()],
            since: Some("{major}.{minor}".into()),
            deprecated: None,
        },
        AttributeRule {
            macro_name: "TEST_DEPRECATED".into(),
            arg_names: vec!["msg".into()],
            since: None,
            deprecated: Some("{msg}".into()),
        },
    ];
    let out = extract_header(&fixture, rules).expect("extraction failed");
    assert!(
        out.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        out.diagnostics
    );

    // Top level: namespace demo plus the three documented macros.
    assert_eq!(out.items.len(), 4, "top level: {:?}", names(&out.items));
    let demo = find(&out.items, "demo");
    assert_eq!(demo.kind, ItemKind::Namespace);
    assert_eq!(demo.qualified_name, "demo");

    // Widget: docs, since from TEST_API(2, 3), public+protected only.
    let widget = find(&demo.children, "Widget");
    assert_eq!(widget.kind, ItemKind::Class);
    assert_eq!(widget.id, "cpp:demo::Widget");
    assert_eq!(widget.since.as_deref(), Some("2.3"));
    let docs = widget.docs.as_ref().expect("Widget docs");
    assert_eq!(docs.summary.as_deref(), Some("A widget that frobs."));
    assert_eq!(docs.see_also, vec!["Gadget"]);
    assert!(
        !widget.children.iter().any(|c| c.name.starts_with("hidden")),
        "private members leaked: {:?}",
        names(&widget.children)
    );

    // frob: signature flags, params, defaults, doc params.
    let frob = find(&widget.children, "frob");
    assert_eq!(frob.kind, ItemKind::Method);
    assert_eq!(frob.access, Some(Access::Public));
    let sig = frob.signature.as_ref().expect("frob signature");
    assert_eq!(sig.return_type.as_deref(), Some("unsigned long"));
    assert!(sig.is_virtual && sig.is_pure_virtual && sig.is_const);
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].name.as_deref(), Some("amount"));
    assert_eq!(sig.params[1].name.as_deref(), Some("fast"));
    assert_eq!(sig.params[1].default.as_deref(), Some("true"));
    let frob_docs = frob.docs.as_ref().expect("frob docs");
    assert_eq!(frob_docs.params.len(), 2);
    assert_eq!(frob_docs.returns.as_deref(), Some("the frob count"));

    // old_frob: deprecation via macro rule.
    let old_frob = find(&widget.children, "old_frob");
    assert_eq!(old_frob.deprecated.as_deref(), Some("Use frob"));

    // Braces inside a macro argument make clang refuse to associate the
    // doc comment (its intervening-text scan trips on `{}`); udg
    // recovers it from the spelled source.
    let old_frob2 = find(&widget.children, "old_frob2");
    assert_eq!(
        old_frob2.deprecated.as_deref(),
        Some("Use frob_{fast,slow}")
    );
    assert_eq!(
        old_frob2.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Even older way to frob.")
    );

    // Deleted functions are not API: the copy constructor and copy
    // assignment vanish, while `= default` (~Widget) and pure-virtual
    // `= 0` (frob) members stay.
    assert!(
        !widget
            .children
            .iter()
            .any(|c| c.kind == ItemKind::Constructor),
        "deleted copy ctor leaked: {:?}",
        names(&widget.children)
    );
    assert!(!widget.children.iter().any(|c| c.name == "operator="));
    assert!(widget.children.iter().any(|c| c.name == "~Widget"));

    // helper: protected, noexcept.
    let helper = find(&widget.children, "helper");
    assert_eq!(helper.access, Some(Access::Protected));
    assert!(helper.signature.as_ref().unwrap().is_noexcept);

    // An undocumented override inherits the base declaration's docs,
    // with provenance; members with their own docs are untouched.
    let steel = find(&demo.children, "SteelWidget");
    let sfrob = find(&steel.children, "frob");
    assert_eq!(
        sfrob.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Frob the widget.")
    );
    assert_eq!(sfrob.docs_from.as_deref(), Some("Widget::frob"));
    let many = find(&steel.children, "frob_many");
    assert_eq!(
        many.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Frob, but in bulk.")
    );
    assert_eq!(many.docs_from, None);

    // Mode: enum with underlying type, values, deprecated enumerator.
    let mode = find(&demo.children, "Mode");
    assert_eq!(mode.kind, ItemKind::Enum);
    assert_eq!(mode.ty.as_deref(), Some("unsigned char"));
    assert_eq!(find(&mode.children, "On").value.as_deref(), Some("0"));
    assert_eq!(find(&mode.children, "Off").value.as_deref(), Some("1"));
    // Spelled initializers win over computed values.
    let legacy = find(&mode.children, "Legacy");
    assert_eq!(legacy.value.as_deref(), Some("Off"));
    assert_eq!(legacy.deprecated.as_deref(), Some("Use Off"));

    // Free function.
    let make = find(&demo.children, "make_widget");
    assert_eq!(make.kind, ItemKind::Function);
    assert_eq!(
        make.signature.as_ref().unwrap().return_type.as_deref(),
        Some("Widget *")
    );

    // Class template.
    let holder = find(&demo.children, "Holder");
    assert_eq!(holder.kind, ItemKind::Class);
    assert_eq!(
        holder
            .signature
            .as_ref()
            .and_then(|s| s.template_params.as_deref()),
        Some("template<typename T, unsigned long N>")
    );
    let get = find(&holder.children, "get");
    assert_eq!(
        get.signature.as_ref().unwrap().return_type.as_deref(),
        Some("T")
    );
    // A constructor template is a constructor (libclang hands it over as
    // a FunctionTemplate), and ctor/dtor names inside a class template
    // lose clang's `<T, N>` spelling.
    let ctor = find(&holder.children, "Holder");
    assert_eq!(ctor.kind, ItemKind::Constructor);
    assert_eq!(ctor.qualified_name, "demo::Holder::Holder");
    assert_eq!(
        ctor.signature.as_ref().unwrap().params[0].name.as_deref(),
        Some("parts")
    );
    let dtor = find(&holder.children, "~Holder");
    assert_eq!(dtor.kind, ItemKind::Destructor);
    // A dependent initializer shows its spelled expression, not clang's
    // meaningless computed 0.
    let cap = find(&holder.children, "CAP");
    assert_eq!(cap.kind, ItemKind::Enumerator);
    assert_eq!(cap.value.as_deref(), Some("N"));

    // Type alias.
    let alias = find(&demo.children, "WidgetPtr");
    assert_eq!(alias.kind, ItemKind::TypeAlias);
    assert_eq!(alias.ty.as_deref(), Some("Widget *"));

    // Unnamed enums hoist their enumerators into the enclosing scope,
    // with the wrapper's docs on constants that lack their own.
    assert!(
        !widget.children.iter().any(|c| c.kind == ItemKind::Enum),
        "unnamed enum wrapper leaked: {:?}",
        names(&widget.children)
    );
    let block = find(&widget.children, "BLOCK_SIZE");
    assert_eq!(block.kind, ItemKind::Enumerator);
    assert_eq!(block.qualified_name, "demo::Widget::BLOCK_SIZE");
    assert_eq!(block.value.as_deref(), Some("16"));
    assert_eq!(block.access, Some(Access::Public));
    assert_eq!(
        block.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Fixed frob geometry.")
    );
    let key = find(&widget.children, "KEY_SIZE");
    assert_eq!(key.value.as_deref(), Some("32"));
    assert_eq!(
        key.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Bytes of key material."),
        "a trailing /**< */ doc must win over the wrapper's"
    );

    // Same at namespace scope.
    let ws = find(&demo.children, "WORKSPACE_SIZE");
    assert_eq!(ws.kind, ItemKind::Enumerator);
    assert_eq!(ws.qualified_name, "demo::WORKSPACE_SIZE");
    assert_eq!(ws.value.as_deref(), Some("8"));
    assert_eq!(
        ws.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Scratch space needed by frobbers.")
    );

    // A typedef'd unnamed enum takes the typedef's name and is NOT hoisted.
    let level = find(&demo.children, "level_t");
    assert_eq!(level.kind, ItemKind::Enum);
    assert_eq!(
        find(&level.children, "LEVEL_HIGH").value.as_deref(),
        Some("2")
    );

    // Macros in documented headers are items: docs from adjacent or
    // trailing comments, params for function-like ones. Attribute-rule
    // macros are markup, and undocumented empty defines (guards) are
    // presence-only, not items.
    let limit = find(&out.items, "WIDGET_LIMIT");
    assert_eq!(limit.kind, ItemKind::Macro);
    assert_eq!(limit.value.as_deref(), Some("64"));
    assert_eq!(
        limit.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Maximum widgets per frob.")
    );
    let pad_default = find(&out.items, "WIDGET_DEFAULT_PAD");
    assert_eq!(
        pad_default.docs.as_ref().and_then(|d| d.summary.as_deref()),
        Some("Pad bytes when unspecified.")
    );
    let pad = find(&out.items, "WIDGET_PAD");
    let psig = pad.signature.as_ref().expect("function-like params");
    assert_eq!(psig.params[0].name.as_deref(), Some("n"));
    assert!(
        pad.value
            .as_deref()
            .unwrap_or("")
            .contains("WIDGET_DEFAULT_PAD")
    );
    assert_eq!(
        pad.docs.as_ref().map(|d| d.params.len()),
        Some(1),
        "macro @param docs"
    );
    for absent in ["TEST_API", "TEST_DEPRECATED", "WIDGET_GUARD_H_"] {
        assert!(
            !out.items.iter().any(|i| i.name == absent),
            "{absent} must not be an item"
        );
    }

    // --- External reference headers (second extraction; libclang
    // handles sequential sessions fine, just not concurrent ones).
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let out = udg::frontend::clang::extract(&udg::frontend::clang::CppConfig {
        std: "c++20".into(),
        c_mode: false,
        language: None,
        include_dirs: vec![dir.clone()],
        defines: vec![],
        extra_args: vec![],
        headers: vec![dir.join("uses_external.hpp")],
        external_headers: vec![dir.join("external_spec.h")],
        attributes: vec![],
    })
    .expect("external extraction");
    // Documented tree holds only the wrapper...
    assert!(out.items.iter().any(|i| i.name == "Wrapper"));
    assert!(!out.items.iter().any(|i| i.name == "CK_THING"));
    // ...while spec names and macros exist for reference resolution.
    for name in ["CK_THING", "CKA_TEST"] {
        assert!(
            out.reference_names.iter().any(|n| n == name),
            "missing reference name {name}: {:?}",
            out.reference_names
        );
    }
    assert!(!out.reference_names.iter().any(|n| n == "_CKA_INTERNAL"));
    // A macro in the DOCUMENTED header is a real item, not a reference.
    assert!(
        out.items
            .iter()
            .any(|i| i.name == "WRAPPER_FLAG" && i.kind == ItemKind::Macro)
    );
}
