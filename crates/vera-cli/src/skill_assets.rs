//! Embedded skill files that `vera agent install` writes to agent skill dirs.

pub struct SkillFile {
    pub relative_path: &'static str,
    pub contents: &'static str,
}

pub const VERA_SKILL_NAME: &str = "vera";

pub const VERA_SKILL_FILES: &[SkillFile] = &[
    SkillFile {
        relative_path: "SKILL.md",
        contents: include_str!("../../../skills/vera/SKILL.md"),
    },
    SkillFile {
        relative_path: "references/install.md",
        contents: include_str!("../../../skills/vera/references/install.md"),
    },
    SkillFile {
        relative_path: "references/query-patterns.md",
        contents: include_str!("../../../skills/vera/references/query-patterns.md"),
    },
    SkillFile {
        relative_path: "references/troubleshooting.md",
        contents: include_str!("../../../skills/vera/references/troubleshooting.md"),
    },
    SkillFile {
        relative_path: "references/mcp.md",
        contents: include_str!("../../../skills/vera/references/mcp.md"),
    },
    SkillFile {
        relative_path: "agents/openai.yaml",
        contents: include_str!("../../../skills/vera/agents/openai.yaml"),
    },
];

#[cfg(test)]
mod tests {
    use super::VERA_SKILL_FILES;

    #[test]
    fn skill_frontmatter_plain_values_have_no_yaml_separators() {
        let frontmatter = VERA_SKILL_FILES[0]
            .contents
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .expect("SKILL.md starts with frontmatter")
            .0;
        for line in frontmatter.lines() {
            let (key, value) = line.split_once(": ").expect("key: value line");
            let quoted = value.len() > 1
                && (value.starts_with('\'') && value.ends_with('\'')
                    || value.starts_with('"') && value.ends_with('"'));
            // A plain YAML scalar cannot contain ": " or " #".
            assert!(
                quoted || !(value.contains(": ") || value.contains(" #")),
                "frontmatter `{key}` needs quoting"
            );
        }
    }
}
