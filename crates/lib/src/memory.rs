use crate::llm::{ChatMessage, ChatRole};

/// Conversation memory manager.
#[derive(Debug, Clone)]
pub struct ConversationMemory {
    messages: Vec<ChatMessage>,
    max_messages: usize,
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
        }
    }

    /// Add a message to the conversation
    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
        self.trim_messages();
    }

    /// Trim messages to max capacity
    fn trim_messages(&mut self) {
        if self.messages.len() > self.max_messages {
            // Keep system messages at the beginning
            let system_messages: Vec<_> = self
                .messages
                .iter()
                .filter(|m| m.role == ChatRole::System)
                .cloned()
                .collect();

            // Calculate how many non-system messages to keep
            let non_system_to_keep = self.max_messages.saturating_sub(system_messages.len());

            // Get all non-system messages
            let all_non_system: Vec<_> = self
                .messages
                .iter()
                .filter(|m| m.role != ChatRole::System)
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

    /// Get all messages
    pub fn get_messages(&self) -> Vec<ChatMessage> {
        self.messages.clone()
    }

    /// Get the last N messages
    pub fn get_last_messages(&self, n: usize) -> Vec<ChatMessage> {
        let start = self.messages.len().saturating_sub(n);
        self.messages[start..].to_vec()
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Get the number of messages
    pub fn len(&self) -> usize {
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
        assert_eq!(memory.len(), 3);

        let messages = memory.get_messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ChatRole::System);
        assert_eq!(messages[1].content, "Message 3"); // Second-to-last
        assert_eq!(messages[2].content, "Message 4"); // Last
    }
}
