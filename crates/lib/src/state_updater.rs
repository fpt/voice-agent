use crate::state_capsule::StateCapsule;
use anyhow::Result;

/// Lightweight state updater for backchannel events
/// This is the "軽量器" that updates state during backchannel moments
/// without polluting the message history
pub trait StateUpdater: Send + Sync {
    /// Update state capsule based on user utterance
    /// Returns updated capsule
    fn update(&self, prev: &StateCapsule, utterance: &str) -> Result<StateCapsule>;

    /// Detect if utterance should trigger a backchannel response
    /// Returns Some(backchannel_text) if triggered, None otherwise
    fn should_backchannel(&self, utterance: &str, pause_ms: u64) -> Option<String>;
}

/// Rule-based state updater (lightweight, no model required)
/// This is the initial implementation - can be replaced with small model later
pub struct RuleBasedStateUpdater {
    /// Enable debug logging
    debug: bool,
}

impl RuleBasedStateUpdater {
    pub fn new() -> Self {
        Self { debug: false }
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Extract keywords for entity detection
    fn extract_entities(&self, utterance: &str) -> Vec<(String, String)> {
        let mut entities = Vec::new();
        let lower = utterance.to_lowercase();

        // Timer-related
        if lower.contains("timer") || lower.contains("タイマー") {
            entities.push(("timer_requested".to_string(), "true".to_string()));
        }

        // Step progression
        if lower.contains("next") || lower.contains("次") {
            entities.push(("step_progression".to_string(), "next".to_string()));
        }
        if lower.contains("previous") || lower.contains("前") || lower.contains("戻") {
            entities.push(("step_progression".to_string(), "previous".to_string()));
        }

        // Completion indicators
        if lower.contains("done")
            || lower.contains("finished")
            || lower.contains("完了")
            || lower.contains("できた")
        {
            entities.push(("completion_status".to_string(), "done".to_string()));
        }

        // Questions
        if lower.contains("?")
            || lower.contains("？")
            || lower.contains("how")
            || lower.contains("どう")
        {
            entities.push(("has_question".to_string(), "true".to_string()));
        }

        entities
    }

    /// Detect intent from utterance
    fn detect_intent(&self, utterance: &str, prev_intent: &str) -> String {
        let lower = utterance.to_lowercase();

        // Timer request
        if lower.contains("timer") || lower.contains("タイマー") {
            return "timer_request".to_string();
        }

        // Step help
        if lower.contains("next") || lower.contains("次") || lower.contains("how") {
            return "step_help_request".to_string();
        }

        // Clarification
        if lower.contains("?")
            || lower.contains("？")
            || lower.contains("what")
            || lower.contains("何")
        {
            return "clarification_request".to_string();
        }

        // Status update (user reporting what they did)
        if lower.contains("done")
            || lower.contains("did")
            || lower.contains("finished")
            || lower.contains("た")
            || lower.contains("完了")
        {
            return "status_update".to_string();
        }

        // Default: keep previous intent
        prev_intent.to_string()
    }

    /// Detect tone from utterance
    fn detect_tone(&self, utterance: &str) -> Option<String> {
        let lower = utterance.to_lowercase();

        if lower.contains("help") || lower.contains("困") || lower.contains("わからない") {
            return Some("confused".to_string());
        }

        if lower.contains("!")
            || lower.contains("！")
            || lower.contains("great")
            || lower.contains("いい")
        {
            return Some("satisfied".to_string());
        }

        if lower.contains("wrong")
            || lower.contains("error")
            || lower.contains("違")
            || lower.contains("エラー")
        {
            return Some("frustrated".to_string());
        }

        None // Keep existing tone
    }
}

impl Default for RuleBasedStateUpdater {
    fn default() -> Self {
        Self::new()
    }
}

impl StateUpdater for RuleBasedStateUpdater {
    fn update(&self, prev: &StateCapsule, utterance: &str) -> Result<StateCapsule> {
        let mut capsule = prev.clone();

        // Update intent
        let new_intent = self.detect_intent(utterance, &prev.intent);
        capsule.set_intent(new_intent);

        // Update entities
        for (key, value) in self.extract_entities(utterance) {
            capsule.set_entity(key, value);
        }

        // Update tone if detected
        if let Some(tone) = self.detect_tone(utterance) {
            capsule.set_tone(tone);
        }

        // Update goals based on intent
        match capsule.intent.as_str() {
            "timer_request" => {
                capsule.add_goal("set_timer");
                capsule.add_open_slot("timer_minutes?");
            }
            "step_help_request" => {
                capsule.add_goal("understand_next_step");
            }
            "status_update" => {
                // User completed something - remove related goals
                capsule.remove_goal("understand_next_step");
                capsule.remove_open_slot("confirm_next_step?");
            }
            _ => {}
        }

        // Maintain reasonable confidence (rule-based is less confident than model-based)
        capsule.set_confidence(0.7);

        if self.debug {
            tracing::debug!(
                "State updated: intent={}, entities={:?}",
                capsule.intent,
                capsule.entities
            );
        }

        Ok(capsule)
    }

    fn should_backchannel(&self, utterance: &str, pause_ms: u64) -> Option<String> {
        // Trigger backchannel if:
        // 1. Short utterance (< 10 words) with pause > 500ms
        // 2. Ends with conjunction words
        // 3. Mid-sentence pause

        let word_count = utterance.split_whitespace().count();
        let lower = utterance.to_lowercase();

        // Short status updates deserve acknowledgment
        if word_count < 10 && pause_ms > 500 {
            // Check for acknowledgment-worthy content
            if lower.contains("done") || lower.contains("finished") || lower.contains("できた") {
                return Some("got it".to_string());
            }

            if lower.contains("mixed")
                || lower.contains("added")
                || lower.contains("溶いた")
                || lower.contains("入れた")
            {
                return Some("mm-hmm".to_string());
            }
        }

        // Conjunction words (user likely continuing)
        if pause_ms > 400 {
            if lower.ends_with("and")
                || lower.ends_with("but")
                || lower.ends_with("so")
                || lower.ends_with("で")
                || lower.ends_with("が")
            {
                return Some("uh-huh".to_string());
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_entities() {
        let updater = RuleBasedStateUpdater::new();
        let entities = updater.extract_entities("I'm done with this step. What's next?");

        assert!(entities
            .iter()
            .any(|(k, v)| k == "completion_status" && v == "done"));
        assert!(entities
            .iter()
            .any(|(k, v)| k == "step_progression" && v == "next"));
        assert!(entities
            .iter()
            .any(|(k, v)| k == "has_question" && v == "true"));
    }

    #[test]
    fn test_detect_intent() {
        let updater = RuleBasedStateUpdater::new();

        assert_eq!(
            updater.detect_intent("Set a timer for 5 minutes", "initial"),
            "timer_request"
        );
        assert_eq!(
            updater.detect_intent("What should I do next?", "initial"),
            "step_help_request"
        );
        assert_eq!(
            updater.detect_intent("I finished mixing the eggs", "initial"),
            "status_update"
        );
    }

    #[test]
    fn test_should_backchannel() {
        let updater = RuleBasedStateUpdater::new();

        // Short status with pause
        assert_eq!(
            updater.should_backchannel("I'm done", 600),
            Some("got it".to_string())
        );

        // Mid-sentence pause
        assert_eq!(
            updater.should_backchannel("I mixed the eggs and", 450),
            Some("uh-huh".to_string())
        );

        // No backchannel for long utterance
        assert_eq!(
            updater.should_backchannel(
                "This is a very long utterance that should not trigger backchannel",
                600
            ),
            None
        );

        // No backchannel for short pause
        assert_eq!(updater.should_backchannel("I'm done", 300), None);
    }

    #[test]
    fn test_update_state() {
        let updater = RuleBasedStateUpdater::new();
        let prev = StateCapsule::default();

        let capsule = updater.update(&prev, "Set a timer please").unwrap();

        assert_eq!(capsule.intent, "timer_request");
        assert!(capsule.user_goals.contains(&"set_timer".to_string()));
        assert!(capsule.open_slots.contains(&"timer_minutes?".to_string()));
    }
}
