//! LLM data types shared across the crate (conversation messages, tool
//! definitions, token usage). The in-process provider layer was removed when
//! voice-agent became an app-server client; inference now lives in the backend agent.

use serde::{Deserialize, Serialize};

// ============================================================================
// Core types
// ============================================================================

/// Token usage information from an LLM API call
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

impl TokenUsage {
    /// Accumulate usage from another call
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
    }
}

/// Chat message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Image content for multimodal messages
#[derive(Debug, Clone)]
pub struct ImageContent {
    pub base64: String,
    pub media_type: String, // "image/png", "image/jpeg"
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Images attached to this message (for vision models)
    #[serde(skip)]
    pub images: Vec<ImageContent>,
}

impl ChatMessage {
    pub fn user(content: String) -> Self {
        Self {
            role: ChatRole::User,
            content,
            images: vec![],
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
            images: vec![],
        }
    }

    pub fn system(content: String) -> Self {
        Self {
            role: ChatRole::System,
            content,
            images: vec![],
        }
    }
}
