//! Module-map loading (from the configured command or file) and
//! assignment of extracted items to modules by source file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ir::{Item, ItemKind, Module};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::MapSource;

#[derive(Deserialize)]
struct ModuleMapDoc {
    modules: Vec<Module>,
}

/// Module ids from the map turn into slash-separated URL and output
/// path segments; each component must be a plain filename-safe token.
fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.split('/').all(|comp| {
            !comp.is_empty()
                && comp != "."
                && comp != ".."
                && comp
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        });
    if !ok {
        bail!(
            "module map: invalid module id `{id}` \
             (slash-separated components of [A-Za-z0-9._-], no `.` or `..`)"
        );
    }
    Ok(())
}

/// Load a module map from its configured source. Modules with no headers
/// anywhere in their subtree are pruned (nothing to document).
pub fn load(source: &MapSource, base: &Path) -> Result<Vec<Module>> {
    let text = match (&source.command, &source.file) {
        (Some(cmd), None) => {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(base)
                .output()
                .with_context(|| format!("failed to run module map command: {cmd}"))?;
            if !out.status.success() {
                bail!(
                    "module map command failed ({}): {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            String::from_utf8(out.stdout).context("module map output is not UTF-8")?
        }
        (None, Some(file)) => {
            let path = if file.is_absolute() {
                file.clone()
            } else {
                base.join(file)
            };
            std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read module map {}", path.display()))?
        }
        _ => bail!("module map needs exactly one of `command` or `file`"),
    };
    let doc: ModuleMapDoc = serde_json::from_str(&text).context("invalid module map JSON")?;
    let mut modules = doc.modules;

    // Ids become URL segments and filesystem path components verbatim;
    // reject anything that could escape the output directory or break
    // out of an href attribute.
    for m in &modules {
        validate_id(&m.id)?;
        if let Some(p) = &m.parent {
            validate_id(p)?;
        }
    }

    // Structural integrity: nav rendering and header assignment assume
    // unique ids and an acyclic parent forest.
    let mut ids = std::collections::HashSet::new();
    for m in &modules {
        if !ids.insert(m.id.as_str()) {
            bail!("module map: duplicate module id `{}`", m.id);
        }
    }
    let parent_of: HashMap<&str, &str> = modules
        .iter()
        .filter_map(|m| m.parent.as_deref().map(|p| (m.id.as_str(), p)))
        .collect();
    for m in &modules {
        if let Some(p) = &m.parent
            && !ids.contains(p.as_str())
        {
            bail!("module map: `{}` names unknown parent `{p}`", m.id);
        }
        let mut seen = std::collections::HashSet::new();
        let mut cur = m.id.as_str();
        while let Some(&p) = parent_of.get(cur) {
            if !seen.insert(p) {
                bail!("module map: parent cycle through `{p}`");
            }
            cur = p;
        }
    }
    let mut owner: HashMap<&str, &str> = HashMap::new();
    for m in &modules {
        for h in &m.headers {
            if let Some(prev) = owner.insert(h.as_str(), m.id.as_str())
                && prev != m.id
            {
                eprintln!(
                    "udg: warning: header {h} appears in modules `{prev}` and `{}`;                      items are assigned to `{}`",
                    m.id, m.id
                );
            }
        }
    }

    // Prune modules whose whole subtree has no headers.
    let mut has_headers: HashMap<String, bool> = modules
        .iter()
        .map(|m| (m.id.clone(), !m.headers.is_empty()))
        .collect();
    // Propagate child headers up (repeat until fixpoint; trees are shallow).
    loop {
        let mut changed = false;
        for m in &modules {
            if *has_headers.get(&m.id).unwrap_or(&false)
                && let Some(parent) = &m.parent
                && let Some(p) = has_headers.get_mut(parent)
                && !*p
            {
                *p = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    modules.retain(|m| *has_headers.get(&m.id).unwrap_or(&false));
    Ok(modules)
}

/// All headers across the map, deduplicated, in module order.
pub fn all_headers(modules: &[Module]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in modules {
        for h in &m.headers {
            let p = PathBuf::from(h);
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    out
}

/// Assign items to modules by canonical source file. Namespaces span
/// modules and stay unassigned; every other item takes its own file's
/// module and passes it down to members.
pub fn assign(items: &mut [Item], modules: &[Module]) {
    let mut by_file: HashMap<PathBuf, String> = HashMap::new();
    for m in modules {
        for h in &m.headers {
            if let Ok(canon) = PathBuf::from(h).canonicalize() {
                by_file.insert(canon, m.id.clone());
            }
        }
    }
    for item in items.iter_mut() {
        assign_one(item, None, &by_file);
    }
}

fn assign_one(item: &mut Item, inherited: Option<&str>, by_file: &HashMap<PathBuf, String>) {
    if item.kind == ItemKind::Namespace {
        for c in &mut item.children {
            assign_one(c, None, by_file);
        }
        return;
    }
    let own = item
        .source
        .as_ref()
        .and_then(|s| PathBuf::from(&s.file).canonicalize().ok())
        .and_then(|p| by_file.get(&p).cloned());
    item.module = own.or_else(|| inherited.map(str::to_owned));
    let module = item.module.clone();
    for c in &mut item.children {
        assign_one(c, module.as_deref(), by_file);
    }
}

#[cfg(test)]
mod tests {
    use super::{load, validate_id};
    use crate::config::MapSource;

    fn load_json(json: &str) -> anyhow::Result<Vec<crate::ir::Module>> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "udg-modmap-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("map.json");
        std::fs::write(&path, json).unwrap();
        let out = load(
            &MapSource {
                command: None,
                file: Some(path),
            },
            &dir,
        );
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn structural_validation() {
        let m = |id: &str, parent: Option<&str>| {
            format!(
                r#"{{"id":"{id}","name":"{id}","headers":["h"]{}}}"#,
                parent
                    .map(|p| format!(r#","parent":"{p}""#))
                    .unwrap_or_default()
            )
        };
        // Valid two-level map loads.
        let ok = format!(
            r#"{{"modules":[{},{}]}}"#,
            m("a", None),
            m("a/b", Some("a"))
        );
        assert!(load_json(&ok).is_ok());
        // Duplicate ids refused.
        let dup = format!(r#"{{"modules":[{},{}]}}"#, m("a", None), m("a", None));
        assert!(format!("{:#}", load_json(&dup).unwrap_err()).contains("duplicate"));
        // Unknown parent refused.
        let orphan = format!(r#"{{"modules":[{}]}}"#, m("a", Some("ghost")));
        assert!(format!("{:#}", load_json(&orphan).unwrap_err()).contains("unknown parent"));
        // Parent cycles refused.
        let cyc = format!(
            r#"{{"modules":[{},{}]}}"#,
            m("a", Some("b")),
            m("b", Some("a"))
        );
        assert!(format!("{:#}", load_json(&cyc).unwrap_err()).contains("cycle"));
    }

    #[test]
    fn module_id_validation() {
        for ok in ["utils", "pubkey/ec_group", "x9.62", "a_b-c"] {
            assert!(validate_id(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "..",
            "../out",
            "a/../b",
            "a//b",
            "a/",
            "id\"quote",
            "sp ace",
        ] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }
}
