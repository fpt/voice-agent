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
        self.catalog_with_bodies(true)
    }

    /// Catalog with each skill's full prompt, or names and descriptions only.
    ///
    /// The bodies are what cost: our two skills are ~866 tokens inlined against
    /// ~101 for the catalog alone. A backend that registered the skills itself
    /// can hand a body to the model on demand through its own lookup tool, so
    /// inlining them there is paying twice.
    ///
    /// The catalog itself always goes in. Dropping it too would save another
    /// ~100 tokens and reintroduce the failure this whole thread started with —
    /// a model that does not know a skill exists never looks it up.
    pub fn catalog_with_bodies(&self, include_bodies: bool) -> Option<String> {
        let skills = self.skills.read().unwrap();
        if skills.is_empty() {
            return None;
        }
        let mut entries: Vec<&Skill> = skills.values().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let mut out = String::from(if include_bodies {
            "Available skills — apply the relevant one's instructions when it fits the request:\n"
        } else {
            "Available skills — look one up to read its full instructions before applying it:\n"
        });
        for s in entries {
            if include_bodies {
                out.push_str(&format!(
                    "\n## {} — {}\n{}\n",
                    s.name, s.description, s.prompt
                ));
            } else {
                out.push_str(&format!("\n## {} — {}\n", s.name, s.description));
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bodies are the expensive part; the catalog is what makes a skill
    /// discoverable at all. A backend that holds the skills itself gets the
    /// catalog only and fetches a body when it wants one.
    #[test]
    fn catalog_can_omit_bodies_but_never_the_names() {
        let registry = SkillRegistry::new();
        registry.add(
            "desk-activity".into(),
            "Connect the screen to a task board".into(),
            "STEP ONE: call list_windows and report.".into(),
        );

        let full = registry.catalog_with_bodies(true).expect("registered");
        assert!(full.contains("STEP ONE"), "{full}");

        let lean = registry.catalog_with_bodies(false).expect("registered");
        assert!(
            !lean.contains("STEP ONE"),
            "the body must be dropped: {lean}"
        );
        // Still discoverable, and still says what it is for.
        assert!(lean.contains("desk-activity"), "{lean}");
        assert!(
            lean.contains("Connect the screen to a task board"),
            "{lean}"
        );
        // And says how to get the rest.
        assert!(lean.contains("look one up"), "{lean}");

        assert!(lean.len() < full.len());
    }

    /// `catalog()` keeps its old meaning for callers that have not been taught
    /// about the split.
    #[test]
    fn catalog_defaults_to_including_bodies() {
        let registry = SkillRegistry::new();
        registry.add("a".into(), "d".into(), "the body".into());
        assert!(registry.catalog().unwrap().contains("the body"));
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
