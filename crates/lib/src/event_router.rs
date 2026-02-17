//! Event router — debounces watcher events and produces summaries.
//!
//! Accepts events from Claude Code hooks (PostToolUse, Stop) and session JSONL
//! entries. A background thread debounces incoming events and, when the debounce
//! window elapses, summarizes the batch and sends it to a summary channel.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crossbeam::channel::{self, Receiver, Sender};
use serde::Deserialize;

use crate::tool::ToolHandler;
use crate::AgentError;

// ============================================================================
// Event types
// ============================================================================

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

/// User speech input (highest priority — bypasses debounce).
#[derive(Debug, Clone, Deserialize)]
pub struct UserSpeechData {
    pub text: String,
}

/// Priority level for event summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPriority {
    /// User speech — cancel existing TTS and process immediately.
    High,
    /// Claude Code / MCP watcher events — normal processing.
    Normal,
}

/// A summary produced by the EventRouter.
pub struct EventSummary {
    pub text: String,
    pub priority: EventPriority,
}

/// Data from a Claude Code hook event (PostToolUse, Stop, etc.).
#[derive(Debug, Clone, Deserialize)]
pub struct HookEventData {
    /// Hook event type: "PostToolUse", "Stop", etc.
    #[serde(default)]
    pub event: String,
    /// Tool name (if applicable).
    #[serde(default)]
    pub tool_name: Option<String>,
    /// File path affected by the tool.
    #[serde(default)]
    pub file_path: Option<String>,
    /// Session identifier (working directory of Claude Code).
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
    /// Entry type: "user", "assistant", "progress", etc.
    #[serde(rename = "type", default)]
    pub event_type: String,
    /// Tool use blocks extracted from the message.
    #[serde(default)]
    pub tool_uses: Vec<ToolUseEntry>,
    /// Text content from the message.
    #[serde(default)]
    pub text_content: Option<String>,
    /// Session identifier (working directory of Claude Code).
    #[serde(default)]
    pub session_id: Option<String>,
}

// ============================================================================
// Summarizer
// ============================================================================

/// Noise event types to skip during summarization.
const NOISE_TYPES: &[&str] = &[
    "progress",
    "file-history-snapshot",
    "queue-operation",
    "system",
    "result",
    "summary",
];

/// Interesting tools to count in the summary.
const INTERESTING_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "Bash", "Read"];

/// Summarize a batch of watcher events into a concise string.
///
/// Returns `None` if no interesting events were found.
pub fn summarize(events: &[WatcherEvent]) -> Option<String> {
    let mut tool_counts: Vec<(&str, usize)> = Vec::new();
    let mut edited_files: Vec<String> = Vec::new();
    let mut bash_commands: Vec<String> = Vec::new();
    let mut test_results: Vec<String> = Vec::new();
    let mut commit_messages: Vec<String> = Vec::new();
    let mut stop_count = 0usize;

    for event in events {
        match event {
            WatcherEvent::Session(se) => {
                // Skip noise types
                if NOISE_TYPES.iter().any(|n| *n == se.event_type) {
                    continue;
                }

                // Extract tool uses
                for tool_use in &se.tool_uses {
                    let tool_name = &tool_use.name;
                    increment_tool_count(&mut tool_counts, tool_name);

                    // File paths from Write/Edit/Read
                    if let Some(fp) = tool_use.input.get("file_path").and_then(|v| v.as_str()) {
                        let short = basename(fp);
                        if !edited_files.contains(&short) {
                            edited_files.push(short);
                        }
                    }

                    // Bash commands
                    if tool_name == "Bash" {
                        if let Some(cmd) = tool_use.input.get("command").and_then(|v| v.as_str()) {
                            let short: String = cmd.chars().take(80).collect();
                            bash_commands.push(short);
                        }
                    }
                }

                // Check for test results and commit messages in text content
                if let Some(ref text) = se.text_content {
                    if text.contains("passed") && text.contains("failed") {
                        for line in text.lines() {
                            if line.contains("passed") || line.contains("failed") {
                                let short: String = line.chars().take(100).collect();
                                test_results.push(short);
                                break;
                            }
                        }
                    }
                    if text.contains("git commit") || text.contains("Co-Authored-By") {
                        for line in text.lines() {
                            if line.contains("-m ") || line.contains("commit") {
                                let short: String = line.chars().take(80).collect();
                                commit_messages.push(short);
                                break;
                            }
                        }
                    }
                }
            }
            WatcherEvent::UserSpeech(_) => {
                // User speech is handled immediately by the router, not batched.
                continue;
            }
            WatcherEvent::Hook(he) => {
                if he.event == "Stop" {
                    stop_count += 1;
                    continue;
                }

                if let Some(ref tool) = he.tool_name {
                    increment_tool_count(&mut tool_counts, tool);
                }
                if let Some(ref fp) = he.file_path {
                    let short = basename(fp);
                    if !edited_files.contains(&short) {
                        edited_files.push(short);
                    }
                }
            }
        }
    }

    // Build summary parts
    let mut parts: Vec<String> = Vec::new();

    // Tool usage summary (only interesting tools)
    let interesting: Vec<(&str, usize)> = tool_counts
        .iter()
        .filter(|(name, _)| INTERESTING_TOOLS.iter().any(|t| t == name))
        .copied()
        .collect();
    if !interesting.is_empty() {
        let mut sorted = interesting;
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        let tool_summary: Vec<String> = sorted.iter().map(|(n, c)| format!("{} x{}", n, c)).collect();
        parts.push(format!("Tools used: {}", tool_summary.join(", ")));
    }

    // Files
    if !edited_files.is_empty() {
        let file_list: String = edited_files.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        let suffix = if edited_files.len() > 5 {
            format!(" (+{} more)", edited_files.len() - 5)
        } else {
            String::new()
        };
        parts.push(format!("Files: {}{}", file_list, suffix));
    }

    // Bash commands
    if !bash_commands.is_empty() {
        let cmd_list: String = bash_commands.iter().take(3).cloned().collect::<Vec<_>>().join("; ");
        parts.push(format!("Ran: {}", cmd_list));
    }

    // Test results
    if let Some(first) = test_results.first() {
        parts.push(format!("Tests: {}", first));
    }

    // Commits
    if let Some(first) = commit_messages.first() {
        parts.push(format!("Committed: {}", first));
    }

    // Stop events
    if stop_count > 0 {
        parts.push("Claude Code finished responding".to_string());
    }

    if parts.is_empty() {
        return None;
    }

    let mut summary = format!("[Claude Code Update] {}", parts.join(". "));
    if summary.len() > 500 {
        summary.truncate(497);
        summary.push_str("...");
    }
    Some(summary)
}

fn increment_tool_count<'a>(counts: &mut Vec<(&'a str, usize)>, name: &'a str) {
    if let Some(entry) = counts.iter_mut().find(|(n, _)| *n == name) {
        entry.1 += 1;
    } else {
        counts.push((name, 1));
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

// ============================================================================
// EventRouter
// ============================================================================

/// Debounces watcher events and produces prioritized summaries.
///
/// User speech events bypass debounce and are output immediately with `High` priority.
/// Claude Code / MCP events are debounced and output with `Normal` priority.
pub struct EventRouter {
    event_tx: Sender<WatcherEvent>,
    summary_rx: Receiver<EventSummary>,
    _thread: std::thread::JoinHandle<()>,
}

impl EventRouter {
    /// Create a new EventRouter with the given debounce interval.
    pub fn new(debounce: Duration) -> Self {
        let (event_tx, event_rx) = channel::unbounded::<WatcherEvent>();
        let (summary_tx, summary_rx) = channel::unbounded::<EventSummary>();

        let thread = std::thread::Builder::new()
            .name("event-router".to_string())
            .spawn(move || {
                Self::run_loop(event_rx, summary_tx, debounce);
            })
            .expect("failed to spawn event-router thread");

        Self {
            event_tx,
            summary_rx,
            _thread: thread,
        }
    }

    /// Feed an event into the router.
    pub fn feed(&self, event: WatcherEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Feed user speech directly (convenience — avoids JSON round-trip).
    pub fn feed_user_speech(&self, text: &str) {
        self.feed(WatcherEvent::UserSpeech(UserSpeechData {
            text: text.to_string(),
        }));
    }

    /// Drain all available summaries (non-blocking).
    pub fn drain_summaries(&self) -> Vec<EventSummary> {
        let mut out = Vec::new();
        while let Ok(s) = self.summary_rx.try_recv() {
            out.push(s);
        }
        out
    }

    fn run_loop(
        event_rx: Receiver<WatcherEvent>,
        summary_tx: Sender<EventSummary>,
        debounce: Duration,
    ) {
        let mut buffer: Vec<WatcherEvent> = Vec::new();

        loop {
            if buffer.is_empty() {
                // Wait indefinitely for the first event
                match event_rx.recv() {
                    Ok(event) => {
                        if Self::handle_event(event, &mut buffer, &summary_tx) {
                            continue;
                        }
                        break;
                    }
                    Err(_) => break, // Channel closed
                }
            } else {
                // Wait for more events or timeout
                match event_rx.recv_timeout(debounce) {
                    Ok(event) => {
                        if Self::handle_event(event, &mut buffer, &summary_tx) {
                            continue; // Reset the timeout
                        }
                        break;
                    }
                    Err(channel::RecvTimeoutError::Timeout) => {
                        // Debounce window elapsed — flush normal-priority events
                        Self::flush_buffer(&mut buffer, &summary_tx);
                    }
                    Err(channel::RecvTimeoutError::Disconnected) => {
                        // Channel closed — flush remaining
                        Self::flush_buffer(&mut buffer, &summary_tx);
                        break;
                    }
                }
            }
        }
    }

    /// Handle a single event. Returns `true` to continue, `false` to break.
    fn handle_event(
        event: WatcherEvent,
        buffer: &mut Vec<WatcherEvent>,
        summary_tx: &Sender<EventSummary>,
    ) -> bool {
        match event {
            WatcherEvent::UserSpeech(ref data) => {
                // Immediately output user speech — no debounce
                tracing::info!("User speech (high priority): {}", data.text);
                let _ = summary_tx.send(EventSummary {
                    text: data.text.clone(),
                    priority: EventPriority::High,
                });
                true
            }
            other => {
                buffer.push(other);
                true
            }
        }
    }

    fn flush_buffer(buffer: &mut Vec<WatcherEvent>, summary_tx: &Sender<EventSummary>) {
        if buffer.is_empty() {
            return;
        }
        let events: Vec<WatcherEvent> = buffer.drain(..).collect();
        if let Some(text) = summarize(&events) {
            tracing::info!("Event summary: {}", text);
            let _ = summary_tx.send(EventSummary {
                text,
                priority: EventPriority::Normal,
            });
        }
    }
}

// ============================================================================
// ReportEventTool
// ============================================================================

/// MCP tool that accepts watcher events and feeds them into an EventRouter.
pub struct ReportEventTool {
    router: Arc<EventRouter>,
}

impl ReportEventTool {
    pub fn new(router: Arc<EventRouter>) -> Self {
        Self { router }
    }
}

impl ToolHandler for ReportEventTool {
    fn name(&self) -> &str {
        "report_event"
    }

    fn description(&self) -> &str {
        "Report a watcher event (from Claude Code hooks or session monitoring). \
         Events are debounced and summarized automatically."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Event source: 'hook', 'session', or 'user'",
                    "enum": ["hook", "session", "user"]
                },
                "text": {
                    "type": "string",
                    "description": "User speech text (for user events)"
                },
                "event": {
                    "type": "string",
                    "description": "Hook event type (e.g. 'PostToolUse', 'Stop')"
                },
                "tool_name": {
                    "type": "string",
                    "description": "Tool name (for hook events)"
                },
                "file_path": {
                    "type": "string",
                    "description": "File path affected (for hook events)"
                },
                "type": {
                    "type": "string",
                    "description": "Session event type (e.g. 'assistant', 'user')"
                },
                "tool_uses": {
                    "type": "array",
                    "description": "Tool use entries (for session events)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "input": { "type": "object" }
                        }
                    }
                },
                "text_content": {
                    "type": "string",
                    "description": "Text content from the message (for session events)"
                }
            },
            "required": ["source"]
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<String, AgentError> {
        let event: WatcherEvent = serde_json::from_value(args).map_err(|e| {
            AgentError::ParseError(format!("Invalid event: {}", e))
        })?;
        self.router.feed(event);
        Ok("ok".to_string())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_hook_events() {
        let events = vec![
            WatcherEvent::Hook(HookEventData {
                event: "PostToolUse".to_string(),
                tool_name: Some("Write".to_string()),
                file_path: Some("/home/user/project/src/main.rs".to_string()),
                session_id: None,
            }),
            WatcherEvent::Hook(HookEventData {
                event: "PostToolUse".to_string(),
                tool_name: Some("Write".to_string()),
                file_path: Some("/home/user/project/src/lib.rs".to_string()),
                session_id: None,
            }),
            WatcherEvent::Hook(HookEventData {
                event: "PostToolUse".to_string(),
                tool_name: Some("Bash".to_string()),
                file_path: None,
                session_id: None,
            }),
        ];

        let summary = summarize(&events).unwrap();
        assert!(summary.starts_with("[Claude Code Update]"));
        assert!(summary.contains("Write x2"));
        assert!(summary.contains("Bash x1"));
        assert!(summary.contains("main.rs"));
        assert!(summary.contains("lib.rs"));
    }

    #[test]
    fn test_summarize_session_events() {
        let events = vec![WatcherEvent::Session(SessionEventData {
            event_type: "assistant".to_string(),
            tool_uses: vec![
                ToolUseEntry {
                    name: "Edit".to_string(),
                    input: serde_json::json!({"file_path": "/tmp/foo.rs"}),
                },
                ToolUseEntry {
                    name: "Bash".to_string(),
                    input: serde_json::json!({"command": "cargo test --release"}),
                },
            ],
            text_content: None,
            session_id: None,
        })];

        let summary = summarize(&events).unwrap();
        assert!(summary.contains("Edit x1"));
        assert!(summary.contains("Bash x1"));
        assert!(summary.contains("foo.rs"));
        assert!(summary.contains("cargo test --release"));
    }

    #[test]
    fn test_summarize_filters_noise() {
        let events = vec![
            WatcherEvent::Session(SessionEventData {
                event_type: "progress".to_string(),
                tool_uses: vec![],
                text_content: None,
                session_id: None,
            }),
            WatcherEvent::Session(SessionEventData {
                event_type: "system".to_string(),
                tool_uses: vec![],
                text_content: None,
                session_id: None,
            }),
        ];

        assert!(summarize(&events).is_none());
    }

    #[test]
    fn test_summarize_stop_events() {
        let events = vec![WatcherEvent::Hook(HookEventData {
            event: "Stop".to_string(),
            tool_name: None,
            file_path: None,
            session_id: None,
        })];

        let summary = summarize(&events).unwrap();
        assert!(summary.contains("Claude Code finished responding"));
    }

    #[test]
    fn test_summarize_caps_at_500() {
        // Create many diverse events to produce a long summary
        let mut events = Vec::new();
        // Many unique files to fill the file list
        for i in 0..20 {
            events.push(WatcherEvent::Session(SessionEventData {
                event_type: "assistant".to_string(),
                tool_uses: vec![
                    ToolUseEntry {
                        name: "Edit".to_string(),
                        input: serde_json::json!({"file_path": format!("/very/long/directory/path/src/components/deeply/nested/file_component_{}_implementation.rs", i)}),
                    },
                    ToolUseEntry {
                        name: "Bash".to_string(),
                        input: serde_json::json!({"command": format!("cargo test --release --features all-features -- test_very_long_name_{}", i)}),
                    },
                ],
                text_content: Some("51 passed, 0 failed in 3.2s\ngit commit -m \"fix: resolve a very long commit message about something\"".to_string()),
                session_id: None,
            }));
        }

        let summary = summarize(&events).unwrap();
        assert!(summary.len() <= 500, "Summary was {} chars", summary.len());
        assert!(summary.ends_with("..."), "Summary: {}", summary);
    }

    #[test]
    fn test_summarize_test_results() {
        let events = vec![WatcherEvent::Session(SessionEventData {
            event_type: "assistant".to_string(),
            tool_uses: vec![],
            text_content: Some(
                "running tests...\n51 passed, 0 failed\nall done".to_string(),
            ),
            session_id: None,
        })];

        let summary = summarize(&events).unwrap();
        assert!(summary.contains("Tests: 51 passed, 0 failed"));
    }

    #[test]
    fn test_summarize_commit_messages() {
        let events = vec![WatcherEvent::Session(SessionEventData {
            event_type: "assistant".to_string(),
            tool_uses: vec![],
            text_content: Some(
                "Created commit:\ngit commit -m \"fix: resolve auth bug\"\nCo-Authored-By: Claude"
                    .to_string(),
            ),
            session_id: None,
        })];

        let summary = summarize(&events).unwrap();
        assert!(summary.contains("Committed:"));
        assert!(summary.contains("commit"));
    }

    #[test]
    fn test_event_router_debounce() {
        let router = Arc::new(EventRouter::new(Duration::from_millis(100)));

        // Feed events
        router.feed(WatcherEvent::Hook(HookEventData {
            event: "PostToolUse".to_string(),
            tool_name: Some("Write".to_string()),
            file_path: Some("/tmp/test.rs".to_string()),
            session_id: None,
        }));
        router.feed(WatcherEvent::Hook(HookEventData {
            event: "PostToolUse".to_string(),
            tool_name: Some("Edit".to_string()),
            file_path: Some("/tmp/other.rs".to_string()),
            session_id: None,
        }));

        // Wait for debounce
        std::thread::sleep(Duration::from_millis(300));

        let summaries = router.drain_summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].priority, EventPriority::Normal);
        assert!(summaries[0].text.contains("Write x1"));
        assert!(summaries[0].text.contains("Edit x1"));
    }

    #[test]
    fn test_report_event_tool() {
        let router = Arc::new(EventRouter::new(Duration::from_millis(100)));
        let tool = ReportEventTool::new(Arc::clone(&router));

        assert_eq!(tool.name(), "report_event");

        let result = tool
            .call(serde_json::json!({
                "source": "hook",
                "event": "PostToolUse",
                "tool_name": "Write",
                "file_path": "/tmp/test.rs"
            }))
            .unwrap();
        assert_eq!(result, "ok");

        // Wait for debounce and check summary
        std::thread::sleep(Duration::from_millis(300));

        let summaries = router.drain_summaries();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].text.contains("Write"));
    }

    #[test]
    fn test_user_speech_immediate() {
        let router = Arc::new(EventRouter::new(Duration::from_secs(10))); // Long debounce

        router.feed_user_speech("Hello, how are you?");

        // User speech should be available immediately (no debounce)
        std::thread::sleep(Duration::from_millis(50));

        let summaries = router.drain_summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].priority, EventPriority::High);
        assert_eq!(summaries[0].text, "Hello, how are you?");
    }

    #[test]
    fn test_user_speech_does_not_flush_buffer() {
        let router = Arc::new(EventRouter::new(Duration::from_millis(200)));

        // Feed a Claude Code event first
        router.feed(WatcherEvent::Hook(HookEventData {
            event: "PostToolUse".to_string(),
            tool_name: Some("Write".to_string()),
            file_path: Some("/tmp/test.rs".to_string()),
            session_id: None,
        }));

        // Feed user speech — should NOT flush the pending hook event
        std::thread::sleep(Duration::from_millis(20));
        router.feed_user_speech("What are you doing?");

        // User speech arrives immediately
        std::thread::sleep(Duration::from_millis(50));
        let summaries = router.drain_summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].priority, EventPriority::High);
        assert_eq!(summaries[0].text, "What are you doing?");

        // Claude Code event arrives after debounce
        std::thread::sleep(Duration::from_millis(300));
        let summaries = router.drain_summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].priority, EventPriority::Normal);
        assert!(summaries[0].text.contains("Write"));
    }

    #[test]
    fn test_mixed_priority_events() {
        let router = Arc::new(EventRouter::new(Duration::from_millis(100)));

        // Feed hook, user speech, then another hook — all quickly
        router.feed(WatcherEvent::Hook(HookEventData {
            event: "PostToolUse".to_string(),
            tool_name: Some("Edit".to_string()),
            file_path: None,
            session_id: None,
        }));
        router.feed_user_speech("Tell me what's happening");
        router.feed(WatcherEvent::Hook(HookEventData {
            event: "Stop".to_string(),
            tool_name: None,
            file_path: None,
            session_id: None,
        }));

        // Wait for everything to settle
        std::thread::sleep(Duration::from_millis(400));

        let summaries = router.drain_summaries();
        // Should have: user speech (High) + batched hook events (Normal)
        assert!(summaries.len() >= 2, "Got {} summaries", summaries.len());

        let high: Vec<_> = summaries.iter().filter(|s| s.priority == EventPriority::High).collect();
        let normal: Vec<_> = summaries.iter().filter(|s| s.priority == EventPriority::Normal).collect();

        assert_eq!(high.len(), 1);
        assert_eq!(high[0].text, "Tell me what's happening");
        assert_eq!(normal.len(), 1);
        assert!(normal[0].text.contains("Claude Code"));
    }

    #[test]
    fn test_deserialize_user_speech() {
        let json = serde_json::json!({
            "source": "user",
            "text": "hello world"
        });
        let event: WatcherEvent = serde_json::from_value(json).unwrap();
        match event {
            WatcherEvent::UserSpeech(u) => {
                assert_eq!(u.text, "hello world");
            }
            _ => panic!("Expected UserSpeech event"),
        }
    }

    #[test]
    fn test_deserialize_hook_event() {
        let json = serde_json::json!({
            "source": "hook",
            "event": "PostToolUse",
            "tool_name": "Edit",
            "file_path": "/tmp/foo.rs"
        });
        let event: WatcherEvent = serde_json::from_value(json).unwrap();
        match event {
            WatcherEvent::Hook(h) => {
                assert_eq!(h.event, "PostToolUse");
                assert_eq!(h.tool_name.as_deref(), Some("Edit"));
            }
            _ => panic!("Expected Hook event"),
        }
    }

    #[test]
    fn test_deserialize_session_event() {
        let json = serde_json::json!({
            "source": "session",
            "type": "assistant",
            "tool_uses": [
                {"name": "Write", "input": {"file_path": "/tmp/x.rs"}}
            ],
            "text_content": "some output"
        });
        let event: WatcherEvent = serde_json::from_value(json).unwrap();
        match event {
            WatcherEvent::Session(s) => {
                assert_eq!(s.event_type, "assistant");
                assert_eq!(s.tool_uses.len(), 1);
                assert_eq!(s.tool_uses[0].name, "Write");
            }
            _ => panic!("Expected Session event"),
        }
    }
}
