use crate::llm::{ChatMessage, ChatRole};
use crate::state_capsule::StateCapsule;

/// Backchannel marker in message history
/// This is the "⟂" marker mentioned in the design
const BACKCHANNEL_MARKER: &str = "⟂";

/// Message with backchannel flag
#[derive(Debug, Clone)]
struct MessageEntry {
    message: ChatMessage,
    is_backchannel: bool,
}

/// Conversation memory manager with State Capsule
#[derive(Debug, Clone)]
pub struct ConversationMemory {
    messages: Vec<MessageEntry>,
    max_messages: usize,
    /// State capsule for compact context
    pub state_capsule: StateCapsule,
}

impl ConversationMemory {
    /// Create a new conversation memory
    pub fn new() -> Self {
        Self::with_capacity(100)
    }

    /// Create a new conversation memory with specified capacity
    pub fn with_capacity(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
            state_capsule: StateCapsule::default(),
        }
    }

    /// Add a regular message to the conversation
    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(MessageEntry {
            message,
            is_backchannel: false,
        });
        self.trim_messages();
    }

    /// Add a backchannel marker to the conversation
    /// This is for tempo tracking only - doesn't pollute context
    pub fn add_backchannel(&mut self) {
        self.messages.push(MessageEntry {
            message: ChatMessage {
                role: ChatRole::Assistant,
                content: BACKCHANNEL_MARKER.to_string(),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            },
            is_backchannel: true,
        });
        self.trim_messages();
    }

    /// Trim messages to max capacity
    fn trim_messages(&mut self) {
        if self.messages.len() > self.max_messages {
            // Keep system messages at the beginning
            let system_messages: Vec<_> = self
                .messages
                .iter()
                .filter(|e| e.message.role == ChatRole::System)
                .cloned()
                .collect();

            // Calculate how many non-system messages to keep
            let non_system_to_keep = self.max_messages.saturating_sub(system_messages.len());

            // Get all non-system messages
            let all_non_system: Vec<_> = self
                .messages
                .iter()
                .filter(|e| e.message.role != ChatRole::System)
                .cloned()
                .collect();

            // Keep the last N non-system messages
            let total_non_system = all_non_system.len();
            let non_system_messages: Vec<_> = if total_non_system > non_system_to_keep {
                all_non_system
                    .into_iter()
                    .skip(total_non_system - non_system_to_keep)
                    .collect()
            } else {
                all_non_system
            };

            self.messages = system_messages;
            self.messages.extend(non_system_messages);
        }
    }

    /// Get all messages (excluding backchannel markers by default)
    pub fn get_messages(&self) -> Vec<ChatMessage> {
        self.messages
            .iter()
            .filter(|e| !e.is_backchannel)
            .map(|e| e.message.clone())
            .collect()
    }

    /// Get all messages including backchannel markers
    /// Useful for debugging or tempo analysis
    pub fn get_messages_with_backchannels(&self) -> Vec<ChatMessage> {
        self.messages.iter().map(|e| e.message.clone()).collect()
    }

    /// Get the last N messages (excluding backchannel markers)
    pub fn get_last_messages(&self, n: usize) -> Vec<ChatMessage> {
        self.messages
            .iter()
            .filter(|e| !e.is_backchannel)
            .map(|e| e.message.clone())
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Update state capsule (called by state updater)
    pub fn update_state_capsule(&mut self, capsule: StateCapsule) {
        self.state_capsule = capsule;
    }

    /// Get current state capsule
    pub fn get_state_capsule(&self) -> &StateCapsule {
        &self.state_capsule
    }

    /// Get state capsule as prompt fragment for LLM
    pub fn get_state_prompt(&self) -> String {
        self.state_capsule.to_prompt_fragment()
    }

    /// Clear all messages and reset state
    pub fn clear(&mut self) {
        self.messages.clear();
        self.state_capsule.clear();
    }

    /// Get the number of messages (excluding backchannel markers)
    pub fn len(&self) -> usize {
        self.messages.iter().filter(|e| !e.is_backchannel).count()
    }

    /// Get total number of messages including backchannel markers
    pub fn total_len(&self) -> usize {
        self.messages.len()
    }

    /// Check if memory is empty
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

impl Default for ConversationMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_message() {
        let mut memory = ConversationMemory::new();
        memory.add_message(ChatMessage::user("Hello".to_string()));
        assert_eq!(memory.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut memory = ConversationMemory::new();
        memory.add_message(ChatMessage::user("Hello".to_string()));
        memory.clear();
        assert_eq!(memory.len(), 0);
    }

    #[test]
    fn test_max_messages() {
        let mut memory = ConversationMemory::with_capacity(3);

        // Add system message
        memory.add_message(ChatMessage::system("System prompt".to_string()));

        // Add more than capacity
        for i in 0..5 {
            memory.add_message(ChatMessage::user(format!("Message {}", i)));
        }

        // After adding 6 messages total (1 system + 5 user) with capacity 3:
        // - System messages are kept
        // - Last 2 non-system messages are kept (capacity - system_count)
        // - Total: 1 system + 2 user = 3 messages
        assert_eq!(memory.total_len(), 3);
        assert_eq!(memory.len(), 3); // All are non-backchannel

        let messages = memory.get_messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ChatRole::System);
        assert_eq!(messages[1].content, "Message 3"); // Second-to-last
        assert_eq!(messages[2].content, "Message 4"); // Last
    }
}
