//! Reusable **Skills**: modular instructions the agent can apply when they match a task.
//!
//! Skills load from markdown/text files under:
//! - `<workspace>/.clido/skills/`
//! - `~/.clido/skills/`
//! - `[skills] extra-paths` in config (relative paths resolve from the workspace root)
//! - `CLIDO_SKILL_PATHS` — extra directories, `:`-separated (Unix) or `;` on Windows
//!
//! Optional **YAML frontmatter** (between `---` lines) describes metadata; the rest is the skill body.
//!
//! Future: `registry_urls` in config is reserved for remote discovery and versioning (not fetched yet).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config_loader::SkillsSection;

// ── Manifest (frontmatter) ───────────────────────────────────────────────────

/// Deserialize a field that can be either a string or a list of strings.
/// Lists are joined with "\n" to create a single string.
fn deserialize_string_or_list<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrListVisitor;

    impl<'de> serde::de::Visitor<'de> for StringOrListVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(value.to_owned())
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(value)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut items: Vec<String> = Vec::new();
            while let Some(item) = seq.next_element()? {
                items.push(item);
            }
            Ok(items.join("\n"))
        }
    }

    deserializer.deserialize_any(StringOrListVisitor)
}

/// Metadata for a skill. Omitted fields are filled from the file stem or body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Stable id (directory-safe). Defaults to file stem.
    #[serde(default)]
    pub id: String,
    /// Human-readable title.
    #[serde(default)]
    pub name: String,
    /// One-line summary for discovery.
    #[serde(default)]
    pub description: String,
    /// When this skill should be used.
    #[serde(default)]
    pub purpose: String,
    /// What the agent should have or ask for.
    #[serde(default, deserialize_with = "deserialize_string_or_list")]
    pub inputs: String,
    /// What the agent should produce or verify.
    #[serde(default, deserialize_with = "deserialize_string_or_list")]
    pub outputs: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Semver or free-form; reserved for marketplace/versioning.
    #[serde(default)]
    pub version: String,
    /// Arbitrary key/value for skill-specific tuning.
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSourceKind {
    Workspace,
    Global,
    Extra,
}

#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub body: String,
    pub source_path: PathBuf,
    pub source: SkillSourceKind,
}

fn humanize_stem(stem: &str) -> String {
    stem.replace(['-', '_'], " ")
}

/// Best-effort user home for skill paths (`HOME`, then `USERPROFILE` on Windows).
fn home_directory() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

fn append_extra_skill_root(
    out: &mut Vec<(PathBuf, SkillSourceKind)>,
    workspace_root: &Path,
    raw: &str,
) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    let expanded = expand_path_token(raw);
    let pb = if expanded.is_absolute() {
        expanded
    } else {
        workspace_root.join(expanded)
    };
    out.push((pb, SkillSourceKind::Extra));
}

fn fill_manifest_defaults(m: &mut SkillManifest, stem: &str, body_hint: Option<&str>) {
    if m.id.is_empty() {
        m.id = stem.to_string();
    }
    if m.name.is_empty() {
        m.name = humanize_stem(stem);
    }
    if m.description.is_empty() {
        let first = m
            .purpose
            .lines()
            .find(|l| !l.trim().is_empty())
            .or_else(|| body_hint.and_then(|b| b.lines().find(|l| !l.trim().is_empty())))
            .unwrap_or("")
            .trim();
        m.description = if first.len() > 160 {
            format!("{}…", first.chars().take(157).collect::<String>())
        } else {
            first.to_string()
        };
    }
    if m.purpose.is_empty() && !m.description.is_empty() {
        m.purpose = m.description.clone();
    }
}

/// Split optional YAML frontmatter from markdown body.
pub fn parse_skill_document(raw: &str, stem: &str) -> Result<(SkillManifest, String), String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("---") {
        let rest = trimmed.trim_start_matches("---").trim_start();
        if let Some(end) = rest.find("\n---") {
            let yaml_part = &rest[..end];
            let after = &rest[end + "\n---".len()..];
            let body = after.trim_start_matches('-').trim_start().to_string();
            // Try to parse YAML, with auto-fixup on failure
            let m: SkillManifest = serde_yaml::from_str(yaml_part)
                .or_else(|_| {
                    // Auto-fix common YAML issues (unquoted strings with colons)
                    let fixed = fixup_yaml(yaml_part);
                    serde_yaml::from_str(&fixed).map_err(|e| {
                        format!(
                            "skill {stem}: invalid YAML frontmatter: {e}\n\
                             Hint: Quote string values containing colons or special chars.\n\
                             Example: inputs: \"What user needs: a path\""
                        )
                    })
                })
                .map_err(|e| format!("skill {stem}: invalid YAML frontmatter: {e}"))?;
            let mut m: SkillManifest = m;
            fill_manifest_defaults(&mut m, stem, Some(body.as_str()));
            return Ok((m, body));
        }
    }
    let body = trimmed.to_string();
    let mut m = SkillManifest::default();
    fill_manifest_defaults(&mut m, stem, Some(body.as_str()));
    Ok((m, body))
}

/// Auto-fix common YAML issues: quote unquoted string values containing colons or special chars.
fn fixup_yaml(yaml: &str) -> String {
    let mut result = String::new();
    for line in yaml.lines() {
        // Skip empty lines and comments
        if line.is_empty() || line.trim_start().starts_with('#') {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Handle list items (lines starting with -)
        if line.trim_start().starts_with('-') {
            let trimmed = line.trim_start();
            let item_content = trimmed.trim_start_matches('-').trim_start();
            // If item content contains colon or special chars, quote it
            if !item_content.is_empty()
                && !item_content.starts_with('"')
                && !item_content.starts_with("'")
                && (item_content.contains(':')
                    || item_content.contains('[')
                    || item_content.contains(']')
                    || item_content.contains('{')
                    || item_content.contains('}'))
            {
                // Quote the list item content
                let indent = line.len() - line.trim_start().len();
                result.push_str(&" ".repeat(indent));
                result.push_str("- \"");
                result.push_str(item_content);
                result.push('"');
                result.push('\n');
                continue;
            }
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Check if this is a key: value line (not a nested mapping)
        if let Some((key, value)) = line.split_once(':') {
            let key_trimmed = key.trim();
            let value_trimmed = value.trim();

            // Skip if value is already quoted, is a number, is a boolean, or is empty
            if value_trimmed.is_empty()
                || value_trimmed.starts_with('"')
                || value_trimmed.starts_with("'")
                || value_trimmed.parse::<f64>().is_ok()
                || value_trimmed == "true"
                || value_trimmed == "false"
                || value_trimmed == "null"
            {
                result.push_str(line);
                result.push('\n');
                continue;
            }

            // If value contains colon, bracket, or other YAML special chars, quote it
            if value_trimmed.contains(':')
                || value_trimmed.contains('[')
                || value_trimmed.contains(']')
                || value_trimmed.contains('{')
                || value_trimmed.contains('}')
                || value_trimmed.contains('&')
                || value_trimmed.contains('*')
                || value_trimmed.contains('|')
                || value_trimmed.contains('>')
            {
                // Quote the value
                result.push_str(key_trimmed);
                result.push_str(": \"");
                result.push_str(value_trimmed);
                result.push('"');
                result.push('\n');
                continue;
            }
        }

        result.push_str(line);
        result.push('\n');
    }
    result
}

fn is_skill_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("txt"))
        .unwrap_or(false)
}

fn list_skill_paths(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_skill_file(p))
        .collect();
    paths.sort();
    Ok(paths)
}

/// Ordered search roots: workspace, user global, config extras, env extras.
pub fn resolve_skill_directories(
    workspace_root: &Path,
    extra_paths: &[String],
) -> Vec<(PathBuf, SkillSourceKind)> {
    let mut out = Vec::new();
    out.push((
        workspace_root.join(".clido").join("skills"),
        SkillSourceKind::Workspace,
    ));
    if let Some(home) = home_directory() {
        out.push((home.join(".clido").join("skills"), SkillSourceKind::Global));
    }
    for p in extra_paths {
        append_extra_skill_root(&mut out, workspace_root, p);
    }
    let sep = if cfg!(windows) { ';' } else { ':' };
    if let Ok(env_paths) = std::env::var("CLIDO_SKILL_PATHS") {
        for part in env_paths.split(sep) {
            append_extra_skill_root(&mut out, workspace_root, part);
        }
    }
    out
}

fn expand_path_token(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = home_directory() {
            return h.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Load skills from directories; first directory wins per `id` (workspace overrides global).
pub fn discover_skills(
    workspace_root: &Path,
    extra_paths: &[String],
) -> Result<Vec<LoadedSkill>, String> {
    let dirs = resolve_skill_directories(workspace_root, extra_paths);
    let mut by_id: HashMap<String, LoadedSkill> = HashMap::new();
    for (dir, source) in dirs {
        let paths = list_skill_paths(&dir)
            .map_err(|e| format!("read skills dir {}: {e}", dir.display()))?;
        for path in paths {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill");
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let (manifest, body) = parse_skill_document(&raw, stem)?;
            if body.trim().is_empty() {
                continue;
            }
            let id = manifest.id.clone();
            if by_id.contains_key(&id) {
                continue;
            }
            by_id.insert(
                id,
                LoadedSkill {
                    manifest,
                    body,
                    source_path: path,
                    source,
                },
            );
        }
    }
    let mut v: Vec<_> = by_id.into_values().collect();
    v.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok(v)
}

/// True if this skill id would be injected given merged `[skills]` config (ignores on-disk presence).
pub fn is_skill_active_for_config(id: &str, cfg: &SkillsSection) -> bool {
    if cfg.no_skills {
        return false;
    }
    if cfg.disabled.iter().any(|d| d == id) {
        return false;
    }
    if !cfg.enabled.is_empty() && !cfg.enabled.iter().any(|e| e == id) {
        return false;
    }
    true
}

/// Apply `[skills]` enable/disable / no-skills from merged config.
pub fn select_active_skills(mut skills: Vec<LoadedSkill>, cfg: &SkillsSection) -> Vec<LoadedSkill> {
    if cfg.no_skills {
        return Vec::new();
    }
    skills.retain(|s| is_skill_active_for_config(&s.manifest.id, cfg));
    skills
}

fn source_label(s: SkillSourceKind) -> &'static str {
    match s {
        SkillSourceKind::Workspace => "workspace",
        SkillSourceKind::Global => "global",
        SkillSourceKind::Extra => "extra",
    }
}

fn format_one_skill(s: &LoadedSkill) -> String {
    let m = &s.manifest;
    let mut head = format!(
        "### skill: {} · {}\n",
        m.id,
        if m.name.is_empty() { &m.id } else { &m.name }
    );
    if !m.version.is_empty() {
        head.push_str(&format!("- **version:** {}\n", m.version));
    }
    if !m.description.is_empty() {
        head.push_str(&format!("- **summary:** {}\n", m.description));
    }
    if !m.purpose.is_empty() {
        head.push_str(&format!("- **when to use:** {}\n", m.purpose));
    }
    if !m.inputs.is_empty() {
        head.push_str(&format!("- **inputs:** {}\n", m.inputs));
    }
    if !m.outputs.is_empty() {
        head.push_str(&format!("- **outputs:** {}\n", m.outputs));
    }
    if !m.tags.is_empty() {
        head.push_str(&format!("- **tags:** {}\n", m.tags.join(", ")));
    }
    head.push_str(&format!(
        "- **source:** {} (`{}`)\n\n",
        source_label(s.source),
        s.source_path.display()
    ));
    format!("{head}{}", s.body.trim())
}

const GUIDE_AUTO: &str = "\n## Skills (MANDATORY workflow step)\n\
You MUST follow this workflow for EVERY user request:\n\
\n\
**Step 0: Skill Check** — Before planning or acting:\n\
1. Read the `<skills>` block in your system prompt.\n\
2. If any skill's **purpose** or **when to use** field matches the user's request:\n\
   - STOP and explicitly state: \"Using skill `<skill-id>`:\" (e.g., \"Using skill `release`:\")\n\
   - Follow EVERY step in that skill's body exactly as written.\n\
   - Do NOT skip, modify, or invent steps outside the skill.\n\
3. If NO skill matches, proceed with your normal planning workflow.\n\
\n\
Rules:\n\
- You MUST check skills BEFORE any other work.\n\
- You MUST name the skill id when applying one.\n\
- You MUST NOT claim a skill ran if it is not loaded.\n\
- You MAY suggest a relevant skill if one would help but is not loaded.\n";

const GUIDE_MIN: &str = "\n## Skills (MANDATORY workflow step)\n\
Before ANY work: check the `<skills>` block. If a skill matches, state \"Using skill `<id>`:\" and follow its steps exactly.\n";

/// Full `<skills>...</skills>` block for the system prompt, or `None` if nothing active.
pub fn build_skills_prompt_block(skills: &[LoadedSkill], auto_suggest: bool) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let guide = if auto_suggest { GUIDE_AUTO } else { GUIDE_MIN };
    let body: String = skills
        .iter()
        .map(format_one_skill)
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    Some(format!("{guide}\n<skills>\n{body}\n</skills>"))
}

/// Discover, filter, and format in one step (agent startup).
pub fn load_skills_prompt_for_workspace(
    workspace_root: &Path,
    cfg: &SkillsSection,
) -> Result<Option<String>, String> {
    let all = discover_skills(workspace_root, &cfg.extra_paths)?;
    let active = select_active_skills(all, cfg);
    let auto = cfg.auto_suggest.unwrap_or(true);
    Ok(build_skills_prompt_block(&active, auto))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_roundtrip() {
        let raw = r#"---
id: demo
name: Demo Skill
purpose: Testing
inputs: none
outputs: ok
---
# Steps
Do the thing.
"#;
        let (m, body) = parse_skill_document(raw, "filestem").unwrap();
        assert_eq!(m.id, "demo");
        assert_eq!(m.name, "Demo Skill");
        assert!(body.contains("Do the thing"));
    }

    #[test]
    fn parse_no_frontmatter_uses_stem() {
        let (m, body) = parse_skill_document("Hello body", "my_skill").unwrap();
        assert_eq!(m.id, "my_skill");
        assert_eq!(body, "Hello body");
    }

    #[test]
    fn fixup_yaml_quotes_values_with_colons() {
        let yaml =
            "description: Create a React component: write tests\ninputs: path: where to create";
        let fixed = fixup_yaml(yaml);
        assert!(fixed.contains("description: \"Create a React component: write tests\""));
        assert!(fixed.contains("inputs: \"path: where to create\""));
    }

    #[test]
    fn fixup_yaml_preserves_already_quoted_values() {
        let yaml = "description: \"Already quoted: value\"\nid: my-skill";
        let fixed = fixup_yaml(yaml);
        assert!(fixed.contains("description: \"Already quoted: value\""));
        assert!(fixed.contains("id: my-skill"));
    }

    #[test]
    fn parse_skill_auto_fixes_yaml() {
        // This YAML has unquoted colons - should auto-fix and parse successfully
        let raw = "---
id: test-skill
description: A skill with: a colon in description
inputs: What user needs: a path
---
# Test Skill
Body content here.
";
        let result = parse_skill_document(raw, "test");
        assert!(
            result.is_ok(),
            "Should auto-fix YAML and parse: {:?}",
            result
        );
        let (m, body) = result.unwrap();
        assert_eq!(m.id, "test-skill");
        assert!(body.contains("Body content here."));
    }
}
