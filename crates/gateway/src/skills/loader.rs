//! Scan the three skill discovery layers, parse `SKILL.md` files, build
//! a `SkillRegistry`. Errors on individual skills are warn + skip — a
//! malformed `SKILL.md` must never block server startup
//! (M2.5 research notes §1, CC parity).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::skill::{Skill, SkillLayer, SkillRegistry};

#[derive(Debug)]
pub struct SkillLoadError {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for SkillLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

/// Built-in skills shipped with the repo (`harness/skills/`). Resolved
/// relative to the `CARGO_MANIFEST_DIR` so the path holds whether the
/// binary is run from the repo root or installed via `cargo install`.
pub fn builtin_skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("harness")
        .join("skills")
}

/// `~/.leek/skills/`. Falls back to `./.leek-fallback/skills/` if `$HOME`
/// is unset (mostly to keep tests deterministic).
pub fn user_skills_dir() -> PathBuf {
    expand_home("~/.leek/skills/")
}

/// Expand a leading `~` to `$HOME`. Used for both skills and plugins;
/// kept here as the canonical implementation.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
        return PathBuf::from(".leek-fallback").join(rest);
    }
    PathBuf::from(path)
}

/// Load every skill from the three layers + any extra layered dirs
/// (plugins add to `extra` at the user layer). Later entries in `extras`
/// override earlier ones; the explicit project layer overrides everything.
///
/// Each `(dir, layer)` entry is a SKILL-bearing directory. A missing dir
/// is silent — every layer is optional.
pub fn load_layered(layers: &[(&Path, SkillLayer)]) -> (SkillRegistry, Vec<SkillLoadError>) {
    // Insertion order = priority order from low to high. We walk the
    // layers in the given order and let later entries overwrite earlier
    // ones in the merged map.
    let mut merged: HashMap<String, Skill> = HashMap::new();
    let mut errors: Vec<SkillLoadError> = Vec::new();

    for (dir, layer) in layers {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                errors.push(SkillLoadError {
                    path: dir.to_path_buf(),
                    reason: format!("cannot read directory: {e}"),
                });
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            match load_one(&path, &skill_md, *layer) {
                Ok(skill) => {
                    merged.insert(skill.name.clone(), skill);
                }
                Err(e) => errors.push(e),
            }
        }
    }

    (SkillRegistry::new(merged), errors)
}

/// Load one `SKILL.md` into a `Skill`. The directory name is the fallback
/// for `name`; an invalid frontmatter or missing `description` is an
/// error (skip + warn — server still starts).
fn load_one(dir: &Path, skill_md: &Path, layer: SkillLayer) -> Result<Skill, SkillLoadError> {
    let raw = std::fs::read_to_string(skill_md).map_err(|e| SkillLoadError {
        path: skill_md.to_path_buf(),
        reason: format!("read failed: {e}"),
    })?;
    let (frontmatter, body) = split_frontmatter(&raw);

    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let name = frontmatter
        .get("name")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(dir_name);
    if name.is_empty() {
        return Err(SkillLoadError {
            path: skill_md.to_path_buf(),
            reason: "skill has no `name` and the parent directory has no name".into(),
        });
    }
    if !is_valid_skill_name(&name) {
        return Err(SkillLoadError {
            path: skill_md.to_path_buf(),
            reason: format!(
                "invalid skill name '{name}' — must match [a-z0-9-]{{1,64}}"
            ),
        });
    }

    let description = frontmatter
        .get("description")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if description.is_empty() {
        return Err(SkillLoadError {
            path: skill_md.to_path_buf(),
            reason: "missing required `description` frontmatter".into(),
        });
    }

    let allowed_tools = frontmatter
        .get("allowed-tools")
        .map(|s| parse_list(s))
        .unwrap_or_default();

    let disable_model_invocation = frontmatter
        .get("disable-model-invocation")
        .map(|s| parse_bool(s))
        .unwrap_or(false);

    Ok(Skill {
        name,
        description,
        allowed_tools,
        body: body.to_string().into(),
        source_layer: layer,
        disable_model_invocation,
    })
}

/// `[a-z0-9-]{1,64}` per CC frontmatter spec (research notes §1). leek
/// is lenient and accepts uppercase too — but if a name contains weird
/// chars we still reject so the system-prompt index stays scannable.
fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Split a `SKILL.md` into (frontmatter map, body).
///
/// The parser is hand-rolled: full YAML would pull serde_yaml + lexical
/// parsers we don't need. It handles the SKILL.md subset:
///   - scalar values:        `name: foo`
///   - quoted scalars:       `description: "Hello world"`
///   - flat inline arrays:   `allowed-tools: [a, b]`
///   - bool-ish:             `disable-model-invocation: true|false|yes|no`
///   - YAML block scalars:   `description: >` followed by indented lines
///     (folded — newlines become spaces, same as YAML `>` semantics)
pub(crate) fn split_frontmatter(input: &str) -> (HashMap<String, String>, &str) {
    let after_open = if let Some(rest) = input.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = input.strip_prefix("---\r\n") {
        rest
    } else {
        return (HashMap::new(), input);
    };
    let close = after_open.match_indices("---").find(|(i, _)| {
        let at_line_start =
            *i == 0 || after_open.as_bytes().get(i.wrapping_sub(1)) == Some(&b'\n');
        let after = &after_open[*i + 3..];
        let at_line_end =
            after.is_empty() || after.starts_with('\n') || after.starts_with('\r');
        at_line_start && at_line_end
    });
    let Some((close_idx, _)) = close else {
        return (HashMap::new(), input);
    };
    let yaml = &after_open[..close_idx];
    let body_rest = &after_open[close_idx + 3..];
    let body = body_rest.trim_start_matches('\r').trim_start_matches('\n');

    let mut map = HashMap::new();
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            i += 1;
            continue;
        };
        let key = k.trim().to_string();
        let val_marker = v.trim();
        // YAML block scalar — `>` (folded) or `|` (literal). Consume
        // subsequent indented lines until the indent drops back.
        if val_marker == ">" || val_marker == "|" {
            let fold = val_marker == ">";
            let base_indent = indent_of(line);
            let mut collected: Vec<String> = Vec::new();
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                let next_indent = indent_of(next);
                if next.trim().is_empty() {
                    // Both fold and literal record the blank; the post-
                    // processing differs (fold collapses; literal keeps it).
                    collected.push(String::new());
                    i += 1;
                    continue;
                }
                if next_indent <= base_indent {
                    break;
                }
                collected.push(next.trim_end().to_string());
                i += 1;
            }
            // Strip leading indentation common to all collected lines.
            let min_indent = collected
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
                .min()
                .unwrap_or(0);
            let stripped: Vec<String> = collected
                .iter()
                .map(|l| {
                    if l.len() >= min_indent {
                        l[min_indent..].to_string()
                    } else {
                        l.clone()
                    }
                })
                .collect();
            let folded = if fold {
                // YAML `>`: newlines → spaces; consecutive blank lines
                // become a single newline.
                let mut out = String::new();
                let mut prev_blank = false;
                for s in &stripped {
                    if s.trim().is_empty() {
                        if !out.is_empty() && !prev_blank {
                            out.push('\n');
                        }
                        prev_blank = true;
                    } else {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push(' ');
                        }
                        out.push_str(s.trim());
                        prev_blank = false;
                    }
                }
                out
            } else {
                stripped.join("\n")
            };
            map.insert(key, folded.trim().to_string());
            continue;
        }
        // Inline scalar: strip matched quotes; arrays / bools stored as-is.
        let mut val = val_marker.to_string();
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = val[1..val.len() - 1].to_string();
        }
        map.insert(key, val);
        i += 1;
    }
    (map, body)
}

/// Count leading whitespace (spaces + tabs) of a single line. Used by
/// the block-scalar parser to decide when the block ends.
fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Parse a flat list value — `[a, b]` *or* a space-separated string,
/// since CC accepts both forms for `allowed-tools`.
fn parse_list(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let body = if raw.starts_with('[') && raw.ends_with(']') {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    body.split(|c: char| c == ',' || c.is_whitespace())
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\''))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Per-test sandbox; the returned path is the root the test owns.
    fn temp_root(label: &str) -> PathBuf {
        let unique = format!(
            "{}-{}-{}",
            label,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let p = std::env::temp_dir().join(format!("leek-skills-test-{unique}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_skill(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn empty_layers_yield_empty_registry() {
        let (reg, errs) = load_layered(&[]);
        assert!(reg.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn missing_layer_dir_is_silent() {
        let nonexistent = std::env::temp_dir().join("leek-skills-test-nonexistent-xyz123");
        let _ = fs::remove_dir_all(&nonexistent);
        let (reg, errs) = load_layered(&[(&nonexistent, SkillLayer::Builtin)]);
        assert!(reg.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn loads_one_skill_with_full_frontmatter() {
        let root = temp_root("basic");
        write_skill(
            &root,
            "alpha",
            "---\nname: alpha\ndescription: A test skill\nallowed-tools: [web_fetch, corpus_search]\n---\n\nBody here.\n",
        );
        let (reg, errs) = load_layered(&[(root.as_path(), SkillLayer::Builtin)]);
        assert!(errs.is_empty(), "errs: {errs:?}");
        let s = reg.get("alpha").unwrap();
        assert_eq!(s.name, "alpha");
        assert_eq!(s.description, "A test skill");
        assert_eq!(s.allowed_tools, vec!["web_fetch", "corpus_search"]);
        assert_eq!(s.body.as_ref(), "Body here.\n");
        assert_eq!(s.source_layer, SkillLayer::Builtin);
        assert!(!s.disable_model_invocation);
    }

    #[test]
    fn skill_name_falls_back_to_directory_name() {
        let root = temp_root("dirname");
        write_skill(&root, "my-skill", "---\ndescription: dir-named\n---\nBody.");
        let (reg, _) = load_layered(&[(root.as_path(), SkillLayer::User)]);
        let s = reg.get("my-skill").unwrap();
        assert_eq!(s.name, "my-skill");
    }

    #[test]
    fn missing_description_is_an_error_and_skip() {
        let root = temp_root("nodesc");
        write_skill(&root, "broken", "---\nname: broken\n---\nbody");
        write_skill(&root, "ok", "---\ndescription: fine\n---\nbody");
        let (reg, errs) = load_layered(&[(root.as_path(), SkillLayer::Builtin)]);
        // The valid skill loads; the broken one is skipped with a logged error.
        assert!(reg.get("ok").is_some());
        assert!(reg.get("broken").is_none());
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("description"));
    }

    #[test]
    fn disable_model_invocation_is_parsed() {
        let root = temp_root("hide");
        write_skill(
            &root,
            "hidden",
            "---\ndescription: hidden one\ndisable-model-invocation: true\n---\nBody.",
        );
        write_skill(
            &root,
            "shown",
            "---\ndescription: shown one\n---\nBody.",
        );
        let (reg, _) = load_layered(&[(root.as_path(), SkillLayer::Builtin)]);
        assert!(reg.get("hidden").unwrap().disable_model_invocation);
        assert!(!reg.get("shown").unwrap().disable_model_invocation);
        let visible = reg.model_visible();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "shown");
    }

    #[test]
    fn project_layer_overrides_user_overrides_builtin() {
        let builtin = temp_root("layer-b");
        let user = temp_root("layer-u");
        let project = temp_root("layer-p");
        write_skill(
            &builtin,
            "shared",
            "---\nname: shared\ndescription: from builtin\n---\nbuiltin body",
        );
        write_skill(
            &user,
            "shared",
            "---\nname: shared\ndescription: from user\n---\nuser body",
        );
        write_skill(
            &project,
            "shared",
            "---\nname: shared\ndescription: from project\n---\nproject body",
        );
        let (reg, errs) = load_layered(&[
            (builtin.as_path(), SkillLayer::Builtin),
            (user.as_path(), SkillLayer::User),
            (project.as_path(), SkillLayer::Project),
        ]);
        assert!(errs.is_empty());
        let s = reg.get("shared").unwrap();
        assert_eq!(s.source_layer, SkillLayer::Project);
        assert_eq!(s.body.as_ref(), "project body");
    }

    #[test]
    fn invalid_skill_name_rejected() {
        let root = temp_root("badname");
        write_skill(
            &root,
            "weird name with spaces",
            "---\ndescription: hi\n---\nbody",
        );
        let (reg, errs) = load_layered(&[(root.as_path(), SkillLayer::Builtin)]);
        assert!(reg.is_empty());
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("invalid skill name"));
    }

    #[test]
    fn directory_without_skill_md_is_ignored() {
        let root = temp_root("nomdfile");
        std::fs::create_dir_all(root.join("noop")).unwrap();
        std::fs::write(root.join("noop/README.md"), "not a skill").unwrap();
        let (reg, errs) = load_layered(&[(root.as_path(), SkillLayer::Builtin)]);
        assert!(reg.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn parse_list_handles_both_yaml_array_and_space_separated() {
        assert_eq!(parse_list("[a, b, c]"), vec!["a", "b", "c"]);
        assert_eq!(parse_list("a b c"), vec!["a", "b", "c"]);
        assert_eq!(parse_list("[\"x\", 'y']"), vec!["x", "y"]);
        assert!(parse_list("").is_empty());
    }

    #[test]
    fn parse_bool_recognizes_common_forms() {
        assert!(parse_bool("true"));
        assert!(parse_bool("YES"));
        assert!(parse_bool("on"));
        assert!(parse_bool("1"));
        assert!(!parse_bool("false"));
        assert!(!parse_bool("nope"));
        assert!(!parse_bool(""));
    }

    #[test]
    fn block_scalar_folded_description_is_supported() {
        // The shipped equity-valuation / crypto-research skills use
        // `description: >` (folded block scalar). The loader must read
        // them without erroring.
        let raw = "---\nname: foo\ndescription: >\n  First line\n  second line\n  third line.\n---\nBody";
        let (fm, _body) = split_frontmatter(raw);
        let desc = fm.get("description").unwrap();
        assert!(desc.contains("First line"));
        assert!(desc.contains("second line"));
        assert!(desc.contains("third line"));
        // Folded → no newlines (paragraph collapsed).
        assert!(!desc.contains('\n'));
    }

    #[test]
    fn block_scalar_with_blank_lines_paragraphs_separated_by_newline() {
        let raw = "---\ndescription: >\n  First paragraph here.\n\n  Second paragraph here.\n---\nBody";
        let (fm, _) = split_frontmatter(raw);
        let desc = fm.get("description").unwrap();
        // Folded YAML: paragraph break → one newline.
        assert!(desc.contains("First paragraph here."));
        assert!(desc.contains("Second paragraph here."));
        assert_eq!(desc.matches('\n').count(), 1);
    }

    #[test]
    fn block_scalar_literal_preserves_newlines() {
        let raw = "---\ndescription: |\n  Line A\n  Line B\n---\nBody";
        let (fm, _) = split_frontmatter(raw);
        let desc = fm.get("description").unwrap();
        assert_eq!(desc, "Line A\nLine B");
    }

    #[test]
    fn registry_iter_sorted_is_stable_alpha() {
        let root = temp_root("sortorder");
        write_skill(&root, "zeta", "---\ndescription: z\n---\nbody");
        write_skill(&root, "alpha", "---\ndescription: a\n---\nbody");
        write_skill(&root, "mike", "---\ndescription: m\n---\nbody");
        let (reg, _) = load_layered(&[(root.as_path(), SkillLayer::Builtin)]);
        let names: Vec<_> = reg.iter_sorted().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["alpha", "mike", "zeta"]);
    }
}
