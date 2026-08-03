use std::collections::HashMap;
use std::sync::RwLock;

/// A skill is a named prompt template that the agent can look up and apply.
pub struct Skill {
    pub name: String,
    pub description: String,
    pub prompt: String,
}

/// Thread-safe registry of skills.
pub struct SkillRegistry {
    skills: RwLock<HashMap<String, Skill>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new skill.
    pub fn add(&self, name: String, description: String, prompt: String) {
        let mut skills = self.skills.write().unwrap();
        tracing::info!("Registered skill: {}", name);
        skills.insert(
            name.clone(),
            Skill {
                name,
                description,
                prompt,
            },
        );
    }

    /// List all skills as "name: description" lines.
    pub fn list(&self) -> String {
        let skills = self.skills.read().unwrap();
        if skills.is_empty() {
            return "No skills registered.".to_string();
        }
        let mut lines: Vec<String> = skills
            .values()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect();
        lines.sort();
        lines.join("\n")
    }

    /// Get a skill's full prompt by name.
    pub fn get(&self, name: &str) -> Option<String> {
        let skills = self.skills.read().unwrap();
        skills.get(name).map(|s| s.prompt.clone())
    }

    /// Build a catalog string for injection into the backend thread's developer
    /// instructions. Each skill's full prompt is inlined (there is no lookup tool
    /// over the wire — the backend gets everything up front). Returns None if no skills
    /// are registered.
    pub fn catalog(&self) -> Option<String> {
        let skills = self.skills.read().unwrap();
        if skills.is_empty() {
            return None;
        }
        let mut entries: Vec<&Skill> = skills.values().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        // Say plainly that these are already here.
        //
        // The backend has its own skill store behind a `LookupSkill`-style tool,
        // loaded from *its* directories (`~/.config/gallium/skills`,
        // `<cwd>/.claude/skills`, …) — never from voice-agent's `skillPaths`. So
        // that tool reports "none" no matter what is inlined below, and a model
        // that consults it concludes it has nothing: one asked "what tools do you
        // have?", called LookupSkill, was told the set was empty, and answered
        // "I don't have any registered tools or skills" while holding sixteen
        // tools and these two skills in context.
        let mut out = String::from(
            "Available skills — their full instructions are already included here, so use them \
             directly and do not look them up with a tool. A skill-lookup tool, if you have one, \
             reports a different set local to the backend and will not list these. Apply the \
             relevant one when it fits the request:\n",
        );
        for s in entries {
            out.push_str(&format!(
                "\n## {} — {}\n{}\n",
                s.name, s.description, s.prompt
            ));
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog has to stand on its own.
    ///
    /// The backend's skill-lookup tool reads its own directories and will report
    /// an empty set no matter what is inlined here, so the text must tell the
    /// model these are already present and not to go looking. A model that did
    /// look concluded it had no tools at all.
    #[test]
    fn catalog_tells_the_model_not_to_look_skills_up() {
        let registry = SkillRegistry::new();
        registry.add(
            "desk-activity".into(),
            "Connect the screen to a task board".into(),
            "Look at the desktop and report.".into(),
        );
        let catalog = registry.catalog().expect("a skill was registered");

        assert!(catalog.contains("already included"), "{catalog}");
        assert!(catalog.contains("do not look them up"), "{catalog}");
        // The body still has to be there — it is the reason inlining is worth
        // the tokens at all.
        assert!(
            catalog.contains("Look at the desktop and report."),
            "{catalog}"
        );
        assert!(catalog.contains("desk-activity"), "{catalog}");
    }

    #[test]
    fn test_skill_registry() {
        let registry = SkillRegistry::new();
        assert_eq!(registry.list(), "No skills registered.");
        assert!(registry.catalog().is_none());

        registry.add(
            "test-skill".to_string(),
            "A test skill".to_string(),
            "Do the test thing.".to_string(),
        );

        assert!(registry.list().contains("test-skill"));
        assert!(registry.list().contains("A test skill"));
        assert_eq!(
            registry.get("test-skill"),
            Some("Do the test thing.".to_string())
        );
        assert_eq!(registry.get("nonexistent"), None);
    }

    #[test]
    fn catalog_inlines_names_descriptions_and_prompts() {
        let registry = SkillRegistry::new();
        registry.add(
            "greeting".to_string(),
            "Greet the user".to_string(),
            "Say hello warmly.".to_string(),
        );
        let catalog = registry.catalog().unwrap();
        assert!(catalog.contains("greeting"));
        assert!(catalog.contains("Greet the user"));
        assert!(catalog.contains("Say hello warmly."));
    }
}
