pub mod capture;
pub mod event_router;
mod harmony;
mod llm;
#[cfg(feature = "local")]
pub mod llm_local;
pub mod mcp;
pub mod mcp_client;
pub mod mcp_client_http;
pub mod mcp_server;
pub mod mcp_server_http;
mod memory;
pub mod react;
pub mod situation;
pub mod skill;
mod state_capsule;
mod state_updater;
pub mod tool;

use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

pub use capture::CaptureRequest;
pub use event_router::{EventPriority, EventSummary};
pub use harmony::HarmonyTemplate;
pub use llm::{create_provider, ChatMessage, ChatRole};
use tool::ToolAccess;
pub use memory::ConversationMemory;
pub use state_capsule::StateCapsule;
pub use state_updater::{RuleBasedStateUpdater, StateUpdater};

// UniFFI generated code
uniffi::include_scaffolding!("agent");

/// JSON Schema for keyword extraction
fn get_keyword_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "response": {
                "type": "string",
                "description": "Your natural language response to the user"
            },
            "keywords": {
                "type": "array",
                "description": "Important keywords from this conversation for speech recognition context (proper nouns, technical terms, domain-specific words)",
                "items": {
                    "type": "string"
                },
                "maxItems": 10
            }
        },
        "required": ["response", "keywords"],
        "additionalProperties": false
    })
}

/// Parse structured JSON response containing both response text and keywords
fn parse_structured_response(json_str: &str) -> Result<(String, Vec<String>), AgentError> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| AgentError::ParseError(format!("Failed to parse JSON: {}", e)))?;

    let response = parsed["response"]
        .as_str()
        .ok_or_else(|| AgentError::ParseError("Missing 'response' field".to_string()))?
        .to_string();

    let keywords = parsed["keywords"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok((response, keywords))
}

/// Configuration for the agent
pub struct AgentConfig {
    pub model_path: Option<String>,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub use_harmony_template: bool,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    pub language: Option<String>,
    pub working_dir: Option<String>,
    pub reasoning_effort: Option<String>,
    /// If set, enables the event router with this debounce interval in seconds.
    pub watcher_debounce_secs: Option<f64>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
            api_key: None,
            use_harmony_template: true,
            temperature: Some(0.7),
            max_tokens: 2048,
            language: Some("en".to_string()),
            working_dir: None,
            reasoning_effort: None,
            watcher_debounce_secs: None,
        }
    }
}

/// Response from the agent
pub struct AgentResponse {
    pub content: String,
    pub role: String,
    pub is_final: bool,
    pub keywords: Option<Vec<String>>,
    pub reasoning: Option<String>,
}

/// Error types for the agent
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Main agent struct
pub struct Agent {
    config: AgentConfig,
    client: Box<dyn llm::LlmProvider>,
    memory: Arc<Mutex<ConversationMemory>>,
    state_updater: Box<dyn StateUpdater>,
    system_prompt: Arc<Mutex<Option<String>>>,
    tool_registry: tool::ToolRegistry,
    skill_registry: Arc<skill::SkillRegistry>,
    event_router: Option<Arc<event_router::EventRouter>>,
    situation: Arc<situation::SituationMessages>,
    capture_request_rx: crossbeam::channel::Receiver<capture::CaptureRequest>,
    capture_result_tx: crossbeam::channel::Sender<capture::CaptureResult>,
    find_result_tx: crossbeam::channel::Sender<capture::CaptureResult>,
}

// Top-level constructor function for UniFFI
pub fn agent_new(config: AgentConfig) -> Result<Arc<Agent>, AgentError> {
    // Initialize tracing (only once)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    // Create LLM provider
    let client = create_provider(
        config.model_path.clone(),
        config.base_url.clone(),
        config.model.clone(),
        config.api_key.clone(),
        config.temperature,
        config.max_tokens,
        config.reasoning_effort.clone(),
    )
    .map_err(|e| AgentError::ConfigError(e.to_string()))?;

    // Create state updater
    let state_updater: Box<dyn StateUpdater> = Box::new(RuleBasedStateUpdater::new());

    // Create tool registry with built-in tools
    let working_dir = config
        .working_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    tracing::info!("Tool working directory: {}", working_dir.display());
    let skill_registry = Arc::new(skill::SkillRegistry::new());

    // Create event router if watcher is enabled
    let event_router = config.watcher_debounce_secs.map(|secs| {
        let debounce = std::time::Duration::from_secs_f64(secs);
        tracing::info!("Event router enabled with {:.1}s debounce", secs);
        Arc::new(event_router::EventRouter::new(debounce))
    });

    let situation = Arc::new(situation::SituationMessages::default());

    // Create capture bridge
    let capture_bridge = capture::CaptureBridge::new();

    let mut tool_registry = tool::create_default_registry(
        working_dir,
        skill_registry.clone(),
        event_router.clone(),
        situation.clone(),
    );

    // Register capture + find_window tools (shared request channel, separate result channels)
    tool_registry.register(Box::new(capture::CaptureScreenTool::new(
        capture_bridge.request_tx.clone(),
        capture_bridge.capture_result_rx.clone(),
    )));
    tool_registry.register(Box::new(capture::FindWindowTool::new(
        capture_bridge.request_tx.clone(),
        capture_bridge.find_result_rx.clone(),
    )));

    Ok(Arc::new(Agent {
        config,
        client,
        memory: Arc::new(Mutex::new(ConversationMemory::new())),
        state_updater,
        system_prompt: Arc::new(Mutex::new(None)),
        tool_registry,
        skill_registry,
        event_router,
        situation,
        capture_request_rx: capture_bridge.request_rx,
        capture_result_tx: capture_bridge.capture_result_tx,
        find_result_tx: capture_bridge.find_result_tx,
    }))
}

impl Agent {
    /// Process a user input and return the agent's response
    pub fn step(&self, user_input: String) -> Result<AgentResponse, AgentError> {
        let mut memory = self.memory.lock();

        // Update state capsule based on user input
        let prev_capsule = memory.get_state_capsule().clone();
        let updated_capsule = self
            .state_updater
            .update(&prev_capsule, &user_input)
            .map_err(|e| AgentError::InternalError(e.to_string()))?;
        memory.update_state_capsule(updated_capsule);

        // Add user message to memory
        memory.add_message(ChatMessage::user(user_input.clone()));

        // Get conversation context
        let mut messages = memory.get_messages();

        // Prepend custom system prompt if set
        let system_prompt = self.system_prompt.lock().clone();
        if let Some(prompt) = system_prompt {
            messages.insert(0, ChatMessage::system(prompt));
        }

        // Prepend state capsule as system message if not empty and after custom prompt
        let state_prompt = memory.get_state_prompt();
        if !state_prompt.is_empty() {
            let insert_pos = if self.system_prompt.lock().is_some() { 1 } else { 0 };
            messages.insert(insert_pos, ChatMessage::system(state_prompt));
        }

        // Inject skill catalog so LLM knows what skills are available
        if let Some(catalog) = self.skill_registry.catalog() {
            messages.push(ChatMessage::system(catalog));
        }

        // Apply Harmony template if enabled
        let formatted_messages = if self.config.use_harmony_template {
            HarmonyTemplate::format_messages(&messages)
        } else {
            messages.clone()
        };

        // Use ReAct loop if provider supports tools and tools are registered
        let (response_text, keywords, reasoning) = if self.client.supports_tools()
            && !self.tool_registry.is_empty()
        {
            // ReAct loop with tool calling
            let mut react_messages = formatted_messages;
            let (text, reasoning) = react::run(
                self.client.as_ref(),
                &mut react_messages,
                &self.tool_registry,
                None,
            )?;

            (text, Vec::new(), reasoning)
        } else if self.client.supports_structured_output() {
            // Structured output for keyword extraction (no tools)
            let schema = get_keyword_schema();
            let json_response = self
                .client
                .chat_with_schema(&formatted_messages, schema, "conversation_response")
                .map_err(|e| AgentError::NetworkError(e.to_string()))?;
            let (text, keywords) = parse_structured_response(&json_response)?;
            (text, keywords, None)
        } else {
            // Fallback: regular chat (no keywords, no tools)
            let response = self
                .client
                .chat(&formatted_messages)
                .map_err(|e| AgentError::NetworkError(e.to_string()))?;
            (response, Vec::new(), None)
        };

        // Add assistant response to memory
        memory.add_message(ChatMessage::assistant(response_text.clone()));

        Ok(AgentResponse {
            content: response_text,
            role: "assistant".to_string(),
            is_final: true,
            keywords: if keywords.is_empty() { None } else { Some(keywords) },
            reasoning,
        })
    }

    /// Process a backchannel event (audio only, no history pollution)
    pub fn process_backchannel(&self, partial_input: String, pause_ms: u64) -> Option<String> {
        let mut memory = self.memory.lock();

        if let Some(backchannel_text) = self
            .state_updater
            .should_backchannel(&partial_input, pause_ms)
        {
            let prev_capsule = memory.get_state_capsule().clone();
            if let Ok(updated_capsule) = self.state_updater.update(&prev_capsule, &partial_input) {
                memory.update_state_capsule(updated_capsule);
            }

            memory.add_backchannel();

            tracing::debug!("Backchannel triggered: '{}'", backchannel_text);
            return Some(backchannel_text);
        }

        None
    }

    /// Reset the conversation memory
    pub fn reset(&self) {
        let mut memory = self.memory.lock();
        memory.clear();
    }

    /// Get the conversation history as JSON string
    pub fn get_conversation_history(&self) -> String {
        let memory = self.memory.lock();
        serde_json::to_string_pretty(&memory.get_messages()).unwrap_or_default()
    }

    /// Set a custom system prompt for the conversation
    pub fn set_system_prompt(&self, prompt: String) {
        let mut system_prompt = self.system_prompt.lock();
        *system_prompt = Some(prompt);
        tracing::info!("System prompt set");
    }

    /// Register a skill with the agent
    pub fn add_skill(&self, name: String, description: String, prompt: String) {
        self.skill_registry.add(name, description, prompt);
    }

    /// Process user input with only a subset of tools enabled
    pub fn step_with_allowed_tools(
        &self,
        user_input: String,
        allowed_tools: Vec<String>,
    ) -> Result<AgentResponse, AgentError> {
        let mut memory = self.memory.lock();

        // Update state capsule based on user input
        let prev_capsule = memory.get_state_capsule().clone();
        let updated_capsule = self
            .state_updater
            .update(&prev_capsule, &user_input)
            .map_err(|e| AgentError::InternalError(e.to_string()))?;
        memory.update_state_capsule(updated_capsule);

        // Add user message to memory
        memory.add_message(ChatMessage::user(user_input.clone()));

        // Get conversation context
        let mut messages = memory.get_messages();

        // Prepend custom system prompt if set
        let system_prompt = self.system_prompt.lock().clone();
        if let Some(prompt) = system_prompt {
            messages.insert(0, ChatMessage::system(prompt));
        }

        // Prepend state capsule as system message if not empty
        let state_prompt = memory.get_state_prompt();
        if !state_prompt.is_empty() {
            let insert_pos = if self.system_prompt.lock().is_some() { 1 } else { 0 };
            messages.insert(insert_pos, ChatMessage::system(state_prompt));
        }

        // Inject skill catalog
        if let Some(catalog) = self.skill_registry.catalog() {
            messages.push(ChatMessage::system(catalog));
        }

        // Apply Harmony template if enabled
        let formatted_messages = if self.config.use_harmony_template {
            HarmonyTemplate::format_messages(&messages)
        } else {
            messages.clone()
        };

        // Use ReAct loop with filtered tools
        let filtered = self.tool_registry.filtered(&allowed_tools);
        let (response_text, keywords, reasoning) = if self.client.supports_tools()
            && !filtered.is_empty()
        {
            let mut react_messages = formatted_messages;
            let (text, reasoning) = react::run(
                self.client.as_ref(),
                &mut react_messages,
                &filtered,
                None,
            )?;
            (text, Vec::new(), reasoning)
        } else if self.client.supports_structured_output() {
            let schema = get_keyword_schema();
            let json_response = self
                .client
                .chat_with_schema(&formatted_messages, schema, "conversation_response")
                .map_err(|e| AgentError::NetworkError(e.to_string()))?;
            let (text, keywords) = parse_structured_response(&json_response)?;
            (text, keywords, None)
        } else {
            let response = self
                .client
                .chat(&formatted_messages)
                .map_err(|e| AgentError::NetworkError(e.to_string()))?;
            (response, Vec::new(), None)
        };

        // Add assistant response to memory
        memory.add_message(ChatMessage::assistant(response_text.clone()));

        Ok(AgentResponse {
            content: response_text,
            role: "assistant".to_string(),
            is_final: true,
            keywords: if keywords.is_empty() { None } else { Some(keywords) },
            reasoning,
        })
    }

    /// Drain all available event summaries (non-blocking).
    ///
    /// Returns summaries with priority: `High` for user speech (cancel TTS),
    /// `Normal` for Claude Code watcher events.
    pub fn drain_watcher_summaries(&self) -> Vec<event_router::EventSummary> {
        self.event_router
            .as_ref()
            .map(|r| r.drain_summaries())
            .unwrap_or_default()
    }

    /// Feed user speech into the event router (high priority, no debounce).
    pub fn feed_user_speech(&self, text: String) {
        if let Some(ref router) = self.event_router {
            router.feed_user_speech(&text);
        }
    }

    /// Feed a watcher event directly (without MCP).
    pub fn feed_watcher_event(&self, json: String) -> Result<(), AgentError> {
        let router = self.event_router.as_ref().ok_or_else(|| {
            AgentError::ConfigError("Event router not enabled".to_string())
        })?;
        let event: event_router::WatcherEvent = serde_json::from_str(&json)
            .map_err(|e| AgentError::ParseError(format!("Invalid event JSON: {}", e)))?;

        // Push to volatile situation store for read_situation_messages tool
        if let Some((line, source, session_id)) = format_event_for_situation(&event) {
            self.situation.push(line, source, session_id);
        }

        router.feed(event);
        Ok(())
    }

    /// One-shot chat without tools — for event reporting.
    /// If `skill_name` is provided, injects that skill's prompt body.
    /// Otherwise injects the skill catalog as before.
    pub fn chat_once(&self, input: String, skill_name: Option<String>) -> Result<String, AgentError> {
        let mut messages = Vec::new();

        // Use custom system prompt if set
        if let Some(prompt) = self.system_prompt.lock().as_ref() {
            messages.push(ChatMessage::system(prompt.clone()));
        }

        // Inject skill body or catalog
        if let Some(ref name) = skill_name {
            if let Some(prompt) = self.skill_registry.get(name) {
                messages.push(ChatMessage::system(prompt));
            }
        } else if let Some(catalog) = self.skill_registry.catalog() {
            messages.push(ChatMessage::system(catalog));
        }

        messages.push(ChatMessage::system(
            "Respond in 1-2 brief spoken sentences. Do not use any tools. \
             Just summarize what happened concisely."
                .to_string(),
        ));
        messages.push(ChatMessage::user(input));

        let response = self
            .client
            .chat(&messages)
            .map_err(|e| AgentError::NetworkError(e.to_string()))?;

        // Store assistant response in memory (but not the input) so user can ask about it
        self.memory
            .lock()
            .add_message(ChatMessage::assistant(response.clone()));

        Ok(response)
    }

    /// Drain all pending capture requests (Swift polls this).
    pub fn drain_capture_requests(&self) -> Vec<capture::CaptureRequest> {
        let mut requests = Vec::new();
        while let Ok(req) = self.capture_request_rx.try_recv() {
            requests.push(req);
        }
        requests
    }

    /// Submit a capture result from Swift back to the waiting Rust tool.
    /// Routes to the correct channel based on request ID prefix.
    pub fn submit_capture_result(
        &self,
        id: String,
        image_base64: String,
        metadata_json: String,
    ) {
        let result = capture::CaptureResult {
            id: id.clone(),
            image_base64,
            metadata_json,
        };
        if id.starts_with("find_") {
            let _ = self.find_result_tx.send(result);
        } else {
            let _ = self.capture_result_tx.send(result);
        }
    }

    /// Push a situation message from Swift (e.g. periodic window list).
    pub fn push_situation_message(&self, text: String, source: String, session_id: String) {
        self.situation.push(text, source, session_id);
    }
}

/// Format a WatcherEvent as a one-line situation message.
/// Returns `(line, source, session_id)` or `None` for events that shouldn't appear.
fn format_event_for_situation(
    event: &event_router::WatcherEvent,
) -> Option<(String, String, String)> {
    match event {
        event_router::WatcherEvent::Hook(h) => {
            let line = if let Some(ref tool) = h.tool_name {
                if let Some(ref path) = h.file_path {
                    let basename = std::path::Path::new(path)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    format!("[hook] {}: {}", tool, basename)
                } else {
                    format!("[hook] {}", tool)
                }
            } else {
                format!("[hook] {}", h.event)
            };
            let session_id = h.session_id.clone().unwrap_or_default();
            Some((line, "hook".to_string(), session_id))
        }
        event_router::WatcherEvent::Session(s) => {
            if s.tool_uses.is_empty() {
                return None;
            }
            let tools: Vec<&str> = s.tool_uses.iter().map(|t| t.name.as_str()).collect();
            let line = format!("[session] {}: {}", s.event_type, tools.join(", "));
            let session_id = s.session_id.clone().unwrap_or_default();
            Some((line, "session".to_string(), session_id))
        }
        event_router::WatcherEvent::UserSpeech(_) => None, // goes to main conversation
    }
}
