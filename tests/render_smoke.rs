//! Renders a tiny synthetic IR and asserts the load-bearing HTML bits:
//! page set, badges, docs, search index, and escaping behavior.

use std::fs;

use udg::ir::{DocBlock, DocParam, Item, ItemKind, Language, Module, Param, Signature};
use udg::render::{RenderInput, render};

fn item(kind: ItemKind, name: &str, qualified: &str, module: &str) -> Item {
    let mut i = Item::new(Language::Cpp, kind, name, qualified);
    i.module = Some(module.into());
    i
}

#[test]
fn renders_synthetic_site() {
    let modules = vec![Module {
        id: "widgets".into(),
        name: "Widgets".into(),
        brief: Some("All about widgets".into()),
        parent: None,
        headers: vec![],
    }];

    let mut class = item(ItemKind::Class, "Widget", "demo::Widget", "widgets");
    class.since = Some("2.3".into());
    class.bases = vec!["Base<T>".into()];
    class.docs = Some(DocBlock {
        summary: Some("A widget.".into()),
        body: Some(
            "A widget.\n\nUses `vector<uint8_t>` internally.\n\n```cpp\nint frobs = 0; // count\n```\n".into(),
        ),
        ..DocBlock::default()
    });

    let mut method = item(ItemKind::Method, "frob", "demo::Widget::frob", "widgets");
    method.access = Some(udg::ir::Access::Public);
    method.deprecated = Some("Use frob2".into());
    method.signature = Some(Signature {
        return_type: Some("size_t".into()),
        params: vec![Param {
            name: Some("amount".into()),
            ty: "size_t".into(),
            default: None,
        }],
        is_const: true,
        ..Signature::default()
    });
    method.docs = Some(DocBlock {
        summary: Some("Frobs.".into()),
        body: Some("Frobs.".into()),
        params: vec![DocParam {
            name: "amount".into(),
            text: "how much".into(),
        }],
        returns: Some("frob count".into()),
        ..DocBlock::default()
    });
    class.children.push(method);

    // A constant hoisted from an unnamed enum: renders in a Constants
    // table on the class page, not as a nested type.
    let mut block = item(
        ItemKind::Enumerator,
        "BLOCK_SIZE",
        "demo::Widget::BLOCK_SIZE",
        "widgets",
    );
    block.access = Some(udg::ir::Access::Public);
    block.value = Some("16".into());
    block.docs = Some(DocBlock {
        summary: Some("Fixed frob geometry.".into()),
        ..DocBlock::default()
    });
    class.children.push(block);

    // A second class whose ctor takes a Widget: its signature must link.
    let mut gadget = item(ItemKind::Class, "Gadget", "demo::Gadget", "widgets");
    gadget.bases = vec!["Widget".into()];
    let mut ctor = item(
        ItemKind::Constructor,
        "Gadget",
        "demo::Gadget::Gadget",
        "widgets",
    );
    ctor.access = Some(udg::ir::Access::Public);
    ctor.signature = Some(Signature {
        params: vec![Param {
            name: Some("w".into()),
            ty: "const Widget &".into(),
            default: None,
        }],
        ..Signature::default()
    });
    gadget.children.push(ctor);
    // A parameter NAMED like a linkable symbol must not autolink: only
    // type positions do.
    let mut setter = item(
        ItemKind::Method,
        "set_count",
        "demo::Gadget::set_count",
        "widgets",
    );
    setter.access = Some(udg::ir::Access::Public);
    setter.signature = Some(Signature {
        return_type: Some("void".into()),
        params: vec![Param {
            name: Some("Widget".into()),
            ty: "int".into(),
            default: None,
        }],
        ..Signature::default()
    });
    gadget.children.push(setter);

    // A free operator over Widget: must attach to Widget's page.
    let mut op = item(
        ItemKind::Function,
        "operator==",
        "demo::operator==",
        "widgets",
    );
    op.signature = Some(Signature {
        return_type: Some("bool".into()),
        params: vec![
            Param {
                name: Some("a".into()),
                ty: "const Widget &".into(),
                default: None,
            },
            Param {
                name: Some("b".into()),
                ty: "const Widget &".into(),
                default: None,
            },
        ],
        ..Signature::default()
    });

    // A namespace-scope hoisted constant: pages like a variable.
    let mut ws = item(
        ItemKind::Enumerator,
        "WORKSPACE_SIZE",
        "demo::WORKSPACE_SIZE",
        "widgets",
    );
    ws.value = Some("8".into());

    // A macro leaf page.
    let mut mac = item(ItemKind::Macro, "WIDGET_LIMIT", "WIDGET_LIMIT", "widgets");
    mac.value = Some("64".into());

    let mut ns = Item::new(Language::Cpp, ItemKind::Namespace, "demo", "demo");
    ns.children.push(class);
    ns.children.push(gadget);
    ns.children.push(op);
    ns.children.push(ws);
    ns.children.push(mac);

    let out_dir = std::env::temp_dir().join(format!("udg-render-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out_dir);

    let extra_css = out_dir.with_extension("extra.css");
    fs::create_dir_all(out_dir.parent().unwrap()).unwrap();
    fs::write(&extra_css, ":root { --link: rebeccapurple; }").unwrap();

    let items = [ns];
    let result = render(
        &RenderInput {
            project: "Demo",
            reference_names: &[],
            trees: vec![udg::render::TreeInput {
                title: "C++ API",
                prefix: "cpp",
                root_namespace: Some("demo"),
                include_prefix: Some("widgets"),
                modules: &modules,
                items: &items,
            }],
            families: &[],
            guide: None,
            branding: udg::render::Branding {
                extra_css: Some(&extra_css),
                logo: None,
                favicon: None,
                footer: Some("Demo docs footer"),
            },
        },
        &out_dir,
    )
    .expect("render failed");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let class_html = fs::read_to_string(out_dir.join("cpp/widgets/class.Widget.html")).unwrap();
    assert!(class_html.contains("class Widget : public Base&lt;T&gt;"));
    assert!(class_html.contains("since 2.3"));
    assert!(class_html.contains("size_t frob(size_t amount) const"));
    assert!(class_html.contains("Use frob2"));
    assert!(class_html.contains("<code>amount</code>"));
    assert!(class_html.contains("frob count"));
    // Raw HTML-ish text in docs must be escaped, not swallowed.
    assert!(class_html.contains("vector&lt;uint8_t&gt;"));

    // Hoisted constants: value table on the class page, leaf page plus
    // module "Constants" row at namespace scope.
    assert!(class_html.contains("<h2>Constants</h2>"));
    assert!(class_html.contains("id=\"e.BLOCK_SIZE\""));
    assert!(class_html.contains("Fixed frob geometry."));
    let ws_html =
        fs::read_to_string(out_dir.join("cpp/widgets/const.WORKSPACE_SIZE.html")).unwrap();
    assert!(ws_html.contains("WORKSPACE_SIZE = 8"));
    assert!(ws_html.contains("Constant"));

    // The sidebar ships once per tree as nav.js; pages carry only a
    // placeholder plus the current-page marker.
    let nav_js = fs::read_to_string(out_dir.join("cpp/nav.js")).unwrap();
    assert!(nav_js.starts_with("window.UDG_NAV = "));
    assert!(nav_js.contains("cpp/widgets/index.html"));
    // minijinja escapes `/` in attributes; browsers decode entities.
    assert!(class_html.contains("data-current=\"cpp&#x2f;widgets&#x2f;index.html\""));
    assert!(class_html.contains("nav.js"));
    assert!(
        !class_html.contains("navtree"),
        "nav tree must not be inlined"
    );

    let mac_html = fs::read_to_string(out_dir.join("cpp/widgets/macro.WIDGET_LIMIT.html")).unwrap();
    assert!(mac_html.contains("#define WIDGET_LIMIT 64"));
    assert!(mac_html.contains("Macro"));

    let module_html = fs::read_to_string(out_dir.join("cpp/widgets/index.html")).unwrap();
    assert!(module_html.contains("All about widgets"));
    assert!(module_html.contains("class.Widget.html"));
    assert!(module_html.contains("const.WORKSPACE_SIZE.html"));
    assert!(module_html.contains("macro.WIDGET_LIMIT.html"));

    // The index must be a script assigning a global (file:// compatible),
    // with valid JSON as the payload.
    let index_js = fs::read_to_string(out_dir.join("search-index.js")).unwrap();
    let payload = index_js
        .trim()
        .strip_prefix("window.UDG_SEARCH_INDEX = ")
        .and_then(|s| s.strip_suffix(';'))
        .expect("search-index.js should assign the global");
    let index: serde_json::Value = serde_json::from_str(payload).unwrap();
    let entries = index.as_array().unwrap();
    assert!(entries.iter().any(|e| e["n"] == "Widget"));
    assert!(
        entries
            .iter()
            .any(|e| e["n"] == "frob" && e["u"].as_str().unwrap().contains("#m.frob"))
    );
    assert!(
        entries
            .iter()
            .any(|e| e["n"] == "operator==" && e["q"] == "operator== (Widget)"),
        "type-qualified operator search entry missing"
    );
    assert!(
        entries.iter().any(|e| e["n"] == "BLOCK_SIZE"
            && e["k"] == "const"
            && e["u"].as_str().unwrap().contains("#e.BLOCK_SIZE")),
        "hoisted constant search entry missing"
    );

    // Base class pages list known derived classes.
    assert!(class_html.contains("Derived classes"));
    assert!(class_html.contains("class.Gadget.html\">Gadget</a>"));

    // Derived class pages list the members they inherit, linked to the
    // base page's anchors.
    let gadget_html = fs::read_to_string(out_dir.join("cpp/widgets/class.Gadget.html")).unwrap();
    assert!(gadget_html.contains("Inherited members"));
    assert!(gadget_html.contains("class.Widget.html#m.frob\">frob</a>"));
    assert!(gadget_html.contains("class.Widget.html#e.BLOCK_SIZE\">BLOCK_SIZE</a>"));

    // Signature type names that resolve must link (and not to self);
    // links to same-directory targets minimize to the bare filename.
    assert!(
        gadget_html.contains("<a href=\"class.Widget.html\">Widget</a> &amp; w"),
        "signature type not linked"
    );
    // ...but a parameter merely NAMED `Widget` stays plain text.
    assert!(
        gadget_html.contains("set_count(int Widget)"),
        "parameter name must not autolink"
    );
    assert!(!class_html.contains("class.Widget.html\">Widget</a></h1>"));

    // Free operators attach to the class page (and leave the module
    // function listing) with a type-qualified search entry.
    assert!(class_html.contains("Non-member operators"));
    assert!(class_html.contains("id=\"nm.operator_3D_3D\""));
    assert!(
        fs::metadata(out_dir.join("cpp/widgets/fn.operator_3D_3D.html")).is_err(),
        "attached operator should not get a leaf page"
    );

    // Branding: extra css linked after the builtin, footer present.
    let style_pos = class_html.find("static/style.css").unwrap();
    let extra_pos = class_html.find("static/extra.css").expect("extra css link");
    assert!(extra_pos > style_pos, "extra css must load after builtin");
    assert!(class_html.contains("Demo docs footer"));
    assert!(fs::metadata(out_dir.join("static/extra.css")).is_ok());

    // Fenced code in docs is syntax highlighted.
    assert!(class_html.contains("hl-"), "code block not highlighted");

    // UDG_ROOT must not be HTML-entity-escaped inside the script tag.
    assert!(!class_html.contains("UDG_ROOT = \"..&#x2f;"));

    let _ = fs::remove_dir_all(&out_dir);
    let _ = fs::remove_file(&extra_css);
}

#[test]
fn output_dir_safety() {
    let base = std::env::temp_dir().join(format!("udg-outdir-safety-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let out_dir = base.join("site");

    let modules = vec![udg::ir::Module {
        id: "m".into(),
        name: "M".into(),
        brief: None,
        parent: None,
        headers: vec![],
    }];
    let mut cls = Item::new(Language::Cpp, ItemKind::Class, "A", "A");
    cls.module = Some("m".into());
    let items = [cls];
    let input = RenderInput {
        project: "P",
        reference_names: &[],
        trees: vec![udg::render::TreeInput {
            title: "C++",
            prefix: "cpp",
            root_namespace: None,
            include_prefix: None,
            modules: &modules,
            items: &items,
        }],
        families: &[],
        guide: None,
        branding: udg::render::Branding::default(),
    };

    // A non-empty directory udg did not create is refused untouched.
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(out_dir.join("precious.txt"), "user data").unwrap();
    let err = match render(&input, &out_dir) {
        Ok(_) => panic!("render into a foreign non-empty dir must fail"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("refusing"), "{err}");
    assert!(fs::metadata(out_dir.join("precious.txt")).is_ok());

    // An empty directory is fine, and a rebuild replaces a marked site.
    fs::remove_file(out_dir.join("precious.txt")).unwrap();
    render(&input, &out_dir).expect("render into empty dir");
    assert!(fs::metadata(out_dir.join(".udg-site")).is_ok());
    render(&input, &out_dir).expect("rebuild over marked site");
    assert!(fs::metadata(out_dir.join("cpp/index.html")).is_ok());
    // No staging leftovers.
    assert!(!base.join(".site.udg-staging").exists());
    let _ = fs::remove_dir_all(&base);
}
