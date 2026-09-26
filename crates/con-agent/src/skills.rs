use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// A skill is a named capability with a prompt template.
/// Skills are discovered from SKILL.md files on the filesystem,
/// following the open agent skills ecosystem format (skills.sh).
///
/// Discovery paths (in priority order):
///   1. Project-local: `<cwd>/.con/skills/*/SKILL.md`
///   2. Global user:   `~/.config/con/skills/*/SKILL.md`
///      (`~/.config/con-terminal/skills/*/SKILL.md` on Windows)
///
/// SKILL.md format (YAML frontmatter + markdown body):
/// ```text
/// ---
/// name: my-skill
/// description: What this skill does
/// ---
///
/// # My Skill
/// Prompt template content that the agent will follow...
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    /// Where this skill was loaded from
    pub source: SkillSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SkillSource {
    /// From <cwd>/.con/skills/
    Project(PathBuf),
    /// From the platform global skills directory.
    Global(PathBuf),
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(p) => write!(f, "project:{}", p.display()),
            Self::Global(p) => write!(f, "global:{}", p.display()),
        }
    }
}

/// Registry of available skills, populated by scanning the filesystem.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: BTreeMap<String, Skill>,
}

impl SkillRegistry {
    /// Create an empty registry. Call `scan()` to populate from filesystem.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan for SKILL.md files from configured paths and populate the registry.
    ///
    /// Global paths are scanned first, then project paths.
    /// Later paths override earlier ones on name collision, so project skills
    /// take priority over global skills.
    ///
    /// Returns the number of skills loaded.
    pub fn scan(&mut self, global_dirs: &[PathBuf], project_dirs: &[PathBuf]) -> usize {
        self.skills.clear();

        // Load global first (later entries override earlier)
        for dir in global_dirs {
            self.scan_directory(dir, SkillSource::Global);
        }
        // Then project (overrides global on collision)
        for dir in project_dirs {
            self.scan_directory(dir, SkillSource::Project);
        }

        let count = self.skills.len();
        if count > 0 {
            log::info!("Loaded {} skill(s) from filesystem", count);
        }
        count
    }

    /// Scan a skill root for nested `SKILL.md` entries.
    fn scan_directory<F>(&mut self, dir: &Path, make_source: F)
    where
        F: Fn(PathBuf) -> SkillSource,
    {
        self.scan_directory_recursive(dir, &make_source);
    }

    fn scan_directory_recursive<F>(&mut self, dir: &Path, make_source: &F)
    where
        F: Fn(PathBuf) -> SkillSource,
    {
        let skill_md = dir.join("SKILL.md");
        if let Some(skill) = parse_skill_md(&skill_md, make_source(dir.to_path_buf())) {
            self.skills.insert(skill.name.clone(), skill);
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return, // directory doesn't exist — that's fine
        };

        let mut child_dirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() || !file_type.is_dir() {
                continue;
            }
            child_dirs.push(path);
        }
        child_dirs.sort();

        for child_dir in child_dirs {
            self.scan_directory_recursive(&child_dir, make_source);
        }
    }

    pub fn register(&mut self, skill: Skill) {
        self.skills.insert(skill.name.clone(), skill);
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    /// Return (name, description) pairs for all registered skills.
    pub fn summaries(&self) -> Vec<(String, String)> {
        self.skills
            .values()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

/// Parse a SKILL.md file into a Skill.
///
/// Expected format: YAML frontmatter with `name` and `description`,
/// followed by markdown body used as the prompt template.
fn parse_skill_md(path: &Path, source: SkillSource) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&content)?;

    let name = frontmatter.get("name")?.to_string();
    let description = frontmatter.get("description").cloned().unwrap_or_default();

    if name.is_empty() {
        return None;
    }

    let prompt_template = body.trim().to_string();
    if prompt_template.is_empty() {
        return None;
    }

    Some(Skill {
        name,
        description,
        prompt_template,
        source,
    })
}

/// Minimal frontmatter parser for SKILL.md files.
/// Parses `---` delimited YAML-like key: value pairs.
/// Returns (key-value map, body after frontmatter).
fn parse_frontmatter(content: &str) -> Option<(HashMap<String, String>, String)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }

    // Find closing ---
    let after_open = &content[3..];
    let after_open = after_open
        .strip_prefix('\n')
        .or_else(|| after_open.strip_prefix("\r\n"))?;
    let close_pos = after_open.find("\n---")?;
    let yaml_str = &after_open[..close_pos];
    let body_start = close_pos + 4; // skip \n---
    let body = if body_start < after_open.len() {
        let rest = &after_open[body_start..];
        rest.strip_prefix('\n')
            .or_else(|| rest.strip_prefix("\r\n"))
            .unwrap_or(rest)
    } else {
        ""
    };

    // Simple key: value parsing (sufficient for name + description)
    let mut map = HashMap::new();
    for line in yaml_str.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            // Strip surrounding quotes if present
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                value[1..value.len() - 1].to_string()
            } else {
                value
            };
            map.insert(key, value);
        }
    }

    Some((map, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_basic() {
        let content =
            "---\nname: my-skill\ndescription: Does something\n---\n\n# My Skill\nDo the thing.";
        let (fm, body) = parse_frontmatter(content).unwrap();
        assert_eq!(fm["name"], "my-skill");
        assert_eq!(fm["description"], "Does something");
        assert!(body.contains("Do the thing."));
    }

    #[test]
    fn frontmatter_with_quotes() {
        let content = "---\nname: \"quoted-skill\"\ndescription: 'Has quotes'\n---\n\nDo stuff.";
        let (fm, body) = parse_frontmatter(content).unwrap();
        assert_eq!(fm["name"], "quoted-skill");
        assert_eq!(fm["description"], "Has quotes");
        assert_eq!(body.trim(), "Do stuff.");
    }

    #[test]
    fn no_frontmatter_returns_none() {
        assert!(parse_frontmatter("# Just markdown\nNo frontmatter here.").is_none());
    }

    #[test]
    fn empty_body_returns_none_from_parse_skill() {
        // parse_skill_md needs a real file, but we can test the logic path:
        // if prompt_template is empty after trimming, skill is None
        let content = "---\nname: empty\ndescription: No body\n---\n";
        let (fm, body) = parse_frontmatter(content).unwrap();
        assert_eq!(fm["name"], "empty");
        assert!(body.trim().is_empty());
    }

    #[test]
    fn registry_names_and_summaries_are_sorted_by_name() {
        let mut registry = SkillRegistry::new();
        for name in ["zeta", "alpha", "middle"] {
            registry.register(Skill {
                name: name.to_string(),
                description: format!("{name} description"),
                prompt_template: format!("{name} prompt"),
                source: SkillSource::Global(PathBuf::from("/tmp")),
            });
        }

        assert_eq!(
            registry.names(),
            vec![
                "alpha".to_string(),
                "middle".to_string(),
                "zeta".to_string()
            ]
        );
        assert_eq!(
            registry.summaries(),
            vec![
                ("alpha".to_string(), "alpha description".to_string()),
                ("middle".to_string(), "middle description".to_string()),
                ("zeta".to_string(), "zeta description".to_string())
            ]
        );
    }

    #[test]
    fn scan_finds_nested_skill_directories() {
        let root = std::env::temp_dir().join(format!("con-skill-scan-{}", uuid::Uuid::new_v4()));
        let top_level = root.join("top-level");
        let nested = root.join(".system").join("nested-skill");

        std::fs::create_dir_all(&top_level).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            top_level.join("SKILL.md"),
            "---\nname: top-level\ndescription: Top level\n---\n\nTop level prompt.",
        )
        .unwrap();
        std::fs::write(
            nested.join("SKILL.md"),
            "---\nname: nested-skill\ndescription: Nested\n---\n\nNested prompt.",
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        let loaded = registry.scan(std::slice::from_ref(&root), &[]);

        assert_eq!(loaded, 2);
        assert!(registry.get("top-level").is_some());
        assert!(registry.get("nested-skill").is_some());

        std::fs::remove_dir_all(root).unwrap();
    }
}
