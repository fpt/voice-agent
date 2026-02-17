//! Volatile situation messages — TTL-based store for system events.
//!
//! Events from Claude Code hooks, session JSONL, and MCP tools are pushed here
//! as one-line records. Messages auto-expire after a configurable TTL (default 60s).
//! The ReAct loop reads them via the `read_situation_messages` tool.
//!
//! Each message carries a `session_id` (working directory path) so multiple
//! Claude Code instances can be distinguished.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::tool::ToolHandler;
use crate::AgentError;

/// A single volatile situation message.
#[derive(Debug, Clone)]
pub struct SituationMessage {
    pub text: String,
    pub source: String,
    pub session_id: String,
    pub timestamp: SystemTime,
    created: Instant,
}

/// Volatile message store with time-based expiry.
///
/// Messages auto-expire after `ttl`. No max-count compaction.
/// Thread-safe via internal `Mutex`.
pub struct SituationMessages {
    messages: Mutex<Vec<SituationMessage>>,
    ttl: Duration,
}

impl SituationMessages {
    pub fn new(ttl: Duration) -> Self {
        Self {
            messages: Mutex::new(Vec::new()),
            ttl,
        }
    }

    /// Push a new message and prune expired ones.
    pub fn push(&self, text: String, source: String, session_id: String) {
        let now = Instant::now();
        let mut msgs = self.messages.lock().unwrap();
        msgs.retain(|m| now.duration_since(m.created) < self.ttl);
        msgs.push(SituationMessage {
            text,
            source,
            session_id,
            timestamp: SystemTime::now(),
            created: now,
        });
    }

    /// Return all non-expired messages (oldest first).
    pub fn read_all(&self) -> Vec<SituationMessage> {
        let now = Instant::now();
        let mut msgs = self.messages.lock().unwrap();
        msgs.retain(|m| now.duration_since(m.created) < self.ttl);
        msgs.clone()
    }

    /// Return non-expired messages for a specific session.
    pub fn read_by_session(&self, session_id: &str) -> Vec<SituationMessage> {
        let now = Instant::now();
        let mut msgs = self.messages.lock().unwrap();
        msgs.retain(|m| now.duration_since(m.created) < self.ttl);
        msgs.iter()
            .filter(|m| m.session_id == session_id)
            .cloned()
            .collect()
    }

    /// Count of non-expired messages.
    pub fn count(&self) -> usize {
        let now = Instant::now();
        let msgs = self.messages.lock().unwrap();
        msgs.iter()
            .filter(|m| now.duration_since(m.created) < self.ttl)
            .count()
    }

    /// Distinct session IDs among non-expired messages.
    pub fn session_ids(&self) -> Vec<String> {
        let now = Instant::now();
        let msgs = self.messages.lock().unwrap();
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for m in msgs.iter().filter(|m| now.duration_since(m.created) < self.ttl) {
            if seen.insert(m.session_id.clone()) {
                ids.push(m.session_id.clone());
            }
        }
        ids
    }

    /// Timestamp of the most recent non-expired message.
    pub fn last_timestamp(&self) -> Option<SystemTime> {
        let now = Instant::now();
        let msgs = self.messages.lock().unwrap();
        msgs.iter()
            .rev()
            .find(|m| now.duration_since(m.created) < self.ttl)
            .map(|m| m.timestamp)
    }
}

impl Default for SituationMessages {
    fn default() -> Self {
        Self::new(Duration::from_secs(60))
    }
}

// ============================================================================
// ReadSituationMessagesTool
// ============================================================================

/// Tool that lets the LLM read current situation messages.
pub struct ReadSituationMessagesTool {
    messages: std::sync::Arc<SituationMessages>,
}

impl ReadSituationMessagesTool {
    pub fn new(messages: std::sync::Arc<SituationMessages>) -> Self {
        Self { messages }
    }

    fn format_time(t: SystemTime) -> String {
        let dur = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        let secs = dur.as_secs();
        let h = (secs / 3600) % 24;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }

    /// Extract the last component of a path for display.
    fn session_basename(session_id: &str) -> &str {
        std::path::Path::new(session_id)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(session_id)
    }

    fn format_messages(msgs: &[SituationMessage], show_session: bool) -> String {
        if msgs.is_empty() {
            return "No recent situation messages.".to_string();
        }
        let mut output = format!("{} situation message(s):\n", msgs.len());
        for msg in msgs {
            let time_str = Self::format_time(msg.timestamp);
            if show_session {
                let basename = Self::session_basename(&msg.session_id);
                output.push_str(&format!(
                    "[{}] ({}) [{}] {}\n",
                    time_str, msg.source, basename, msg.text
                ));
            } else {
                output.push_str(&format!("[{}] ({}) {}\n", time_str, msg.source, msg.text));
            }
        }
        output
    }
}

impl ToolHandler for ReadSituationMessagesTool {
    fn name(&self) -> &str {
        "read_situation_messages"
    }

    fn description(&self) -> &str {
        "Read recent system situation messages from Claude Code events, hooks, and MCP tools. \
         Pass session_id (working directory path) to filter by a specific Claude Code session."
    }

    fn dynamic_description(&self) -> Option<String> {
        let count = self.messages.count();
        if count == 0 {
            return None;
        }
        let sessions = self.messages.session_ids();
        let last_str = self
            .messages
            .last_timestamp()
            .map(|t| Self::format_time(t))
            .unwrap_or_else(|| "?".to_string());

        let session_info = if sessions.len() <= 1 {
            String::new()
        } else {
            let names: Vec<&str> = sessions.iter().map(|s| Self::session_basename(s)).collect();
            format!(", sessions: {}", names.join(", "))
        };

        Some(format!(
            "Read recent system situation messages from Claude Code events, hooks, and MCP tools. \
             [{} message{}, last at {}{}]",
            count,
            if count == 1 { "" } else { "s" },
            last_str,
            session_info,
        ))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "Filter by session (working directory path). Omit to read all sessions."
                }
            },
            "required": []
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<String, AgentError> {
        let session_id = args
            .get("session_id")
            .and_then(|v| v.as_str());

        let msgs = if let Some(sid) = session_id {
            self.messages.read_by_session(sid)
        } else {
            self.messages.read_all()
        };

        let sessions = self.messages.session_ids();
        let show_session = sessions.len() > 1 && session_id.is_none();
        Ok(Self::format_messages(&msgs, show_session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_push_and_read() {
        let store = SituationMessages::new(Duration::from_secs(60));
        store.push("[hook] Write: main.rs".into(), "hook".into(), "/project/a".into());
        store.push("[session] assistant: Edit x2".into(), "session".into(), "/project/a".into());
        assert_eq!(store.count(), 2);
        let all = store.read_all();
        assert_eq!(all.len(), 2);
        assert!(all[0].text.contains("main.rs"));
    }

    #[test]
    fn test_ttl_expiry() {
        let store = SituationMessages::new(Duration::from_millis(50));
        store.push("old".into(), "hook".into(), "/p".into());
        thread::sleep(Duration::from_millis(100));
        assert_eq!(store.count(), 0);
        assert!(store.read_all().is_empty());
    }

    #[test]
    fn test_push_prunes_expired() {
        let store = SituationMessages::new(Duration::from_millis(50));
        store.push("old".into(), "hook".into(), "/p".into());
        thread::sleep(Duration::from_millis(100));
        store.push("new".into(), "session".into(), "/p".into());
        assert_eq!(store.count(), 1);
        assert_eq!(store.read_all()[0].text, "new");
    }

    #[test]
    fn test_last_timestamp() {
        let store = SituationMessages::new(Duration::from_secs(60));
        assert!(store.last_timestamp().is_none());
        store.push("first".into(), "hook".into(), "/p".into());
        assert!(store.last_timestamp().is_some());
    }

    #[test]
    fn test_read_by_session() {
        let store = SituationMessages::new(Duration::from_secs(60));
        store.push("a1".into(), "hook".into(), "/project/a".into());
        store.push("b1".into(), "hook".into(), "/project/b".into());
        store.push("a2".into(), "hook".into(), "/project/a".into());

        let a_msgs = store.read_by_session("/project/a");
        assert_eq!(a_msgs.len(), 2);
        assert_eq!(a_msgs[0].text, "a1");
        assert_eq!(a_msgs[1].text, "a2");

        let b_msgs = store.read_by_session("/project/b");
        assert_eq!(b_msgs.len(), 1);
        assert_eq!(b_msgs[0].text, "b1");

        assert!(store.read_by_session("/project/c").is_empty());
    }

    #[test]
    fn test_session_ids() {
        let store = SituationMessages::new(Duration::from_secs(60));
        store.push("a1".into(), "hook".into(), "/project/a".into());
        store.push("b1".into(), "hook".into(), "/project/b".into());
        store.push("a2".into(), "hook".into(), "/project/a".into());

        let ids = store.session_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "/project/a");
        assert_eq!(ids[1], "/project/b");
    }

    #[test]
    fn test_tool_call_empty() {
        let store = Arc::new(SituationMessages::default());
        let tool = ReadSituationMessagesTool::new(store);
        let result = tool.call(serde_json::json!({})).unwrap();
        assert!(result.contains("No recent"));
    }

    #[test]
    fn test_tool_call_with_messages() {
        let store = Arc::new(SituationMessages::default());
        store.push("[hook] Write: foo.rs".into(), "hook".into(), "/p".into());
        let tool = ReadSituationMessagesTool::new(store);
        let result = tool.call(serde_json::json!({})).unwrap();
        assert!(result.contains("1 situation message"));
        assert!(result.contains("foo.rs"));
    }

    #[test]
    fn test_tool_call_filter_by_session() {
        let store = Arc::new(SituationMessages::default());
        store.push("a-event".into(), "hook".into(), "/project/a".into());
        store.push("b-event".into(), "hook".into(), "/project/b".into());
        let tool = ReadSituationMessagesTool::new(store);

        let result = tool.call(serde_json::json!({"session_id": "/project/a"})).unwrap();
        assert!(result.contains("a-event"));
        assert!(!result.contains("b-event"));

        let result = tool.call(serde_json::json!({})).unwrap();
        assert!(result.contains("a-event"));
        assert!(result.contains("b-event"));
        // Multi-session output shows session basename
        assert!(result.contains("[a]"));
        assert!(result.contains("[b]"));
    }

    #[test]
    fn test_dynamic_description() {
        let store = Arc::new(SituationMessages::default());
        let tool = ReadSituationMessagesTool::new(Arc::clone(&store));
        assert!(tool.dynamic_description().is_none());

        store.push("test".into(), "hook".into(), "/p".into());
        let desc = tool.dynamic_description().unwrap();
        assert!(desc.contains("1 message,"));
    }

    #[test]
    fn test_dynamic_description_multi_session() {
        let store = Arc::new(SituationMessages::default());
        let tool = ReadSituationMessagesTool::new(Arc::clone(&store));

        store.push("a".into(), "hook".into(), "/project/alpha".into());
        store.push("b".into(), "hook".into(), "/project/beta".into());

        let desc = tool.dynamic_description().unwrap();
        assert!(desc.contains("2 messages"));
        assert!(desc.contains("sessions: alpha, beta"));
    }
}
