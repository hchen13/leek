//! Walk every plugin root, validate manifests, return what the M2.5
//! runtime loader needs to wire skills + hooks into the registry.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug)]
pub struct PluginLoadError {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for PluginLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

/// On-disk manifest. Only the fields we actually consume — `name`
/// (required) plus optional metadata used in the API. Other CC fields
/// like `repository` / `homepage` are tolerated but ignored.
#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<serde_json::Value>,
}

/// Result of loading one plugin — paths the runtime needs (skills dir,
/// hooks file) plus metadata for `GET /api/v1/plugins`.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub author_display: Option<String>,
    pub root: PathBuf,
    /// `Some(path)` if `<root>/skills/` exists, else `None`.
    pub skills_dir: Option<PathBuf>,
    /// `Some(path)` if `<root>/hooks/hooks.json` exists, else `None`.
    pub hooks_file: Option<PathBuf>,
}

/// Walk each search root and gather every valid plugin. A search root is
/// a *parent* directory: `~/.leek/plugins/` holds many plugin
/// subdirectories. Missing roots are silent — most users won't have any.
/// Malformed manifests are an error per plugin (warn + skip).
pub fn load_all(roots: &[PathBuf]) -> (Vec<LoadedPlugin>, Vec<PluginLoadError>) {
    let mut loaded = Vec::new();
    let mut errors = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(e) => {
                errors.push(PluginLoadError {
                    path: root.clone(),
                    reason: format!("read_dir failed: {e}"),
                });
                continue;
            }
        };
        for entry in entries.flatten() {
            let plugin_root = entry.path();
            if !plugin_root.is_dir() {
                continue;
            }
            let manifest_path = plugin_root.join(".claude-plugin").join("plugin.json");
            if !manifest_path.exists() {
                // Not a plugin dir — could be a README or stray file.
                continue;
            }
            match load_one(&plugin_root, &manifest_path) {
                Ok(p) => loaded.push(p),
                Err(e) => errors.push(e),
            }
        }
    }

    (loaded, errors)
}

fn load_one(root: &Path, manifest_path: &Path) -> Result<LoadedPlugin, PluginLoadError> {
    let raw = std::fs::read_to_string(manifest_path).map_err(|e| PluginLoadError {
        path: manifest_path.to_path_buf(),
        reason: format!("read failed: {e}"),
    })?;
    let manifest: Manifest = serde_json::from_str(&raw).map_err(|e| PluginLoadError {
        path: manifest_path.to_path_buf(),
        reason: format!("invalid plugin.json: {e}"),
    })?;
    if manifest.name.trim().is_empty() {
        return Err(PluginLoadError {
            path: manifest_path.to_path_buf(),
            reason: "manifest `name` is empty".into(),
        });
    }

    let skills_dir = {
        let p = root.join("skills");
        if p.exists() && p.is_dir() {
            Some(p)
        } else {
            None
        }
    };
    let hooks_file = {
        let p = root.join("hooks").join("hooks.json");
        if p.exists() && p.is_file() {
            Some(p)
        } else {
            None
        }
    };

    Ok(LoadedPlugin {
        name: manifest.name,
        description: manifest.description,
        version: manifest.version,
        author_display: manifest.author.map(stringify_author),
        root: root.to_path_buf(),
        skills_dir,
        hooks_file,
    })
}

/// CC `author` can be a string ("Alice") or an object ({"name": ...}).
/// Render either as a flat display string for the API.
fn stringify_author(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Object(map) => map
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(label: &str) -> PathBuf {
        let unique = format!(
            "{}-{}-{}",
            label,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let p = std::env::temp_dir().join(format!("leek-plugins-test-{unique}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_manifest(root: &Path, plugin_name: &str, manifest_json: &str) -> PathBuf {
        let plugin_dir = root.join(plugin_name);
        fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_dir.join(".claude-plugin/plugin.json"),
            manifest_json,
        )
        .unwrap();
        plugin_dir
    }

    #[test]
    fn empty_roots_returns_nothing() {
        let (loaded, errs) = load_all(&[]);
        assert!(loaded.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn nonexistent_root_is_silent() {
        let p = std::env::temp_dir().join("leek-plugins-nonexistent-yyy");
        let _ = fs::remove_dir_all(&p);
        let (loaded, errs) = load_all(&[p]);
        assert!(loaded.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn loads_minimal_plugin() {
        let root = temp_root("min");
        write_manifest(
            &root,
            "tiny",
            r#"{ "name": "tiny", "description": "a tiny plugin", "version": "1.0.0" }"#,
        );
        let (loaded, errs) = load_all(&[root]);
        assert!(errs.is_empty(), "errs: {errs:?}");
        assert_eq!(loaded.len(), 1);
        let p = &loaded[0];
        assert_eq!(p.name, "tiny");
        assert_eq!(p.description.as_deref(), Some("a tiny plugin"));
        assert_eq!(p.version.as_deref(), Some("1.0.0"));
        assert!(p.skills_dir.is_none());
        assert!(p.hooks_file.is_none());
    }

    #[test]
    fn discovers_skills_dir_and_hooks_file() {
        let root = temp_root("full");
        let dir = write_manifest(&root, "rich", r#"{ "name": "rich" }"#);
        fs::create_dir_all(dir.join("skills/example")).unwrap();
        fs::write(
            dir.join("skills/example/SKILL.md"),
            "---\ndescription: hi\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(dir.join("hooks")).unwrap();
        fs::write(dir.join("hooks/hooks.json"), r#"{ "hooks": {} }"#).unwrap();

        let (loaded, _) = load_all(&[root]);
        let p = &loaded[0];
        assert!(p.skills_dir.is_some());
        assert!(p.hooks_file.is_some());
    }

    #[test]
    fn invalid_manifest_is_an_error_not_a_panic() {
        let root = temp_root("invalid");
        write_manifest(&root, "broken", r#"{ not even valid JSON"#);
        write_manifest(&root, "ok", r#"{ "name": "ok" }"#);
        let (loaded, errs) = load_all(&[root]);
        // The good plugin still loads.
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "ok");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("invalid plugin.json"));
    }

    #[test]
    fn empty_name_rejected() {
        let root = temp_root("noname");
        write_manifest(&root, "anon", r#"{ "name": "" }"#);
        let (loaded, errs) = load_all(&[root]);
        assert!(loaded.is_empty());
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("name"));
    }

    #[test]
    fn directory_without_manifest_is_silently_skipped() {
        let root = temp_root("noplugin");
        std::fs::create_dir_all(root.join("random")).unwrap();
        std::fs::write(root.join("random/file.txt"), "stray").unwrap();
        let (loaded, errs) = load_all(&[root]);
        assert!(loaded.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn author_object_renders_to_name_string() {
        let root = temp_root("author-obj");
        write_manifest(
            &root,
            "named-author",
            r#"{ "name": "p", "author": { "name": "Alice", "email": "a@b" } }"#,
        );
        let (loaded, _) = load_all(&[root]);
        assert_eq!(loaded[0].author_display.as_deref(), Some("Alice"));
    }

    #[test]
    fn author_plain_string_passes_through() {
        let root = temp_root("author-str");
        write_manifest(&root, "p", r#"{ "name": "p", "author": "Bob" }"#);
        let (loaded, _) = load_all(&[root]);
        assert_eq!(loaded[0].author_display.as_deref(), Some("Bob"));
    }
}
