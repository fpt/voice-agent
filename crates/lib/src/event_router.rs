//! Watcher event types for parsing Claude Code hook and session JSONL events.

use serde::Deserialize;

/// A watcher event from a Claude Code hook, session JSONL, or user speech.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "source")]
pub enum WatcherEvent {
    #[serde(rename = "hook")]
    Hook(HookEventData),
    #[serde(rename = "session")]
    Session(SessionEventData),
    #[serde(rename = "user")]
    UserSpeech(UserSpeechData),
}

/// User speech — ignored in watcher path (goes directly to step()).
#[derive(Debug, Clone, Deserialize)]
pub struct UserSpeechData {
    pub text: String,
}

/// Data from a Claude Code hook event (PostToolUse, Stop, etc.).
#[derive(Debug, Clone, Deserialize)]
pub struct HookEventData {
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// A tool use entry in a session event.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolUseEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

/// Data from a session JSONL entry.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionEventData {
    #[serde(rename = "type", default)]
    pub event_type: String,
    #[serde(default)]
    pub tool_uses: Vec<ToolUseEntry>,
    #[serde(default)]
    pub text_content: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}
