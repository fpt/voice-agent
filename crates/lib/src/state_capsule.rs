use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// State Capsule: Compact representation of conversation state
/// Kept under 100-200 tokens to maintain dense context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateCapsule {
    /// Current user intent (e.g., "cook_step_help", "timer_request", "clarification")
    pub intent: String,

    /// Extracted entities from conversation (e.g., {"recipe": "omelet", "step": 2})
    #[serde(default)]
    pub entities: HashMap<String, String>,

    /// User's active goals (e.g., ["finish step 2", "set 5-min timer"])
    #[serde(default)]
    pub user_goals: Vec<String>,

    /// Conversation tone (e.g., "neutral", "frustrated", "confused", "satisfied")
    #[serde(default = "default_tone")]
    pub tone: String,

    /// Open slots that need to be filled (e.g., ["confirm_next_step?", "timer_minutes?"])
    #[serde(default)]
    pub open_slots: Vec<String>,

    /// Confidence score of current state (0.0-1.0)
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_tone() -> String {
    "neutral".to_string()
}

fn default_confidence() -> f32 {
    1.0
}

impl Default for StateCapsule {
    fn default() -> Self {
        Self {
            intent: "initial_greeting".to_string(),
            entities: HashMap::new(),
            user_goals: Vec::new(),
            tone: "neutral".to_string(),
            open_slots: Vec::new(),
            confidence: 1.0,
        }
    }
}

impl StateCapsule {
    /// Create a new empty state capsule
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialize to compact JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Format as prompt fragment for LLM
    pub fn to_prompt_fragment(&self) -> String {
        format!(
            "<State Capsule>\n{}\n</State Capsule>",
            serde_json::to_string_pretty(self).unwrap_or_default()
        )
    }

    /// Update intent
    pub fn set_intent(&mut self, intent: impl Into<String>) {
        self.intent = intent.into();
    }

    /// Add or update an entity
    pub fn set_entity(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entities.insert(key.into(), value.into());
    }

    /// Add a user goal
    pub fn add_goal(&mut self, goal: impl Into<String>) {
        let goal = goal.into();
        if !self.user_goals.contains(&goal) {
            self.user_goals.push(goal);
        }
    }

    /// Remove a completed goal
    pub fn remove_goal(&mut self, goal: &str) {
        self.user_goals.retain(|g| g != goal);
    }

    /// Set conversation tone
    pub fn set_tone(&mut self, tone: impl Into<String>) {
        self.tone = tone.into();
    }

    /// Add an open slot
    pub fn add_open_slot(&mut self, slot: impl Into<String>) {
        let slot = slot.into();
        if !self.open_slots.contains(&slot) {
            self.open_slots.push(slot);
        }
    }

    /// Remove a filled slot
    pub fn remove_open_slot(&mut self, slot: &str) {
        self.open_slots.retain(|s| s != slot);
    }

    /// Set confidence score
    pub fn set_confidence(&mut self, confidence: f32) {
        self.confidence = confidence.clamp(0.0, 1.0);
    }

    /// Clear all state (for reset)
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_capsule() {
        let capsule = StateCapsule::default();
        assert_eq!(capsule.intent, "initial_greeting");
        assert_eq!(capsule.tone, "neutral");
        assert_eq!(capsule.confidence, 1.0);
    }

    #[test]
    fn test_set_entity() {
        let mut capsule = StateCapsule::new();
        capsule.set_entity("recipe", "omelet");
        capsule.set_entity("step", "2");

        assert_eq!(capsule.entities.get("recipe"), Some(&"omelet".to_string()));
        assert_eq!(capsule.entities.get("step"), Some(&"2".to_string()));
    }

    #[test]
    fn test_goals() {
        let mut capsule = StateCapsule::new();
        capsule.add_goal("finish step 2");
        capsule.add_goal("set timer");
        capsule.add_goal("finish step 2"); // duplicate should be ignored

        assert_eq!(capsule.user_goals.len(), 2);

        capsule.remove_goal("finish step 2");
        assert_eq!(capsule.user_goals.len(), 1);
        assert_eq!(capsule.user_goals[0], "set timer");
    }

    #[test]
    fn test_prompt_fragment() {
        let mut capsule = StateCapsule::new();
        capsule.set_intent("cook_step_help");
        capsule.set_entity("recipe", "omelet");

        let fragment = capsule.to_prompt_fragment();
        assert!(fragment.contains("<State Capsule>"));
        assert!(fragment.contains("cook_step_help"));
        assert!(fragment.contains("omelet"));
    }
}
