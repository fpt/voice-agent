pub mod app_server_client;
pub mod appserver;
pub mod capture;
pub mod goal;
mod llm;
pub mod mcp;
mod memory;
pub mod situation;
pub mod skill;
pub mod tool;

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam::channel::{Receiver, Sender};
use serde_json::{json, Value};

pub use capture::CaptureRequest;
pub use llm::{ChatMessage, ChatRole, TokenUsage};
pub use memory::ConversationMemory;

// UniFFI generated code
uniffi::include_scaffolding!("agent");

/// Configuration for an external MCP server to spawn and connect to.
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    /// If set, connect over Streamable HTTP to this URL instead of spawning
    /// `command`. (stdio uses command/args; HTTP uses url.)
    pub url: Option<String>,
}

/// Configuration for the agent
pub struct AgentConfig {
    pub model_path: Option<String>,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    pub language: Option<String>,
    pub working_dir: Option<String>,
    pub reasoning_effort: Option<String>,
    /// Local inference backend: "llamacpp" (default) or "gallium". Forwarded to
    /// the backend agent process via `INFERENCE_ENGINE`.
    pub inference_engine: Option<String>,
    /// Backend agent program to spawn: `"gallium"`, `"codex"`, or a full
    /// `"prog arg1 arg2"`. Overridden by the `VOICE_AGENT_BACKEND` env var;
    /// `None` falls back to `gallium`.
    pub backend: Option<String>,
    pub mcp_servers: Vec<McpServerConfig>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5.6-luna".to_string(),
            api_key: None,
            temperature: Some(0.7),
            max_tokens: 2048,
            language: Some("en".to_string()),
            working_dir: None,
            reasoning_effort: None,
            inference_engine: None,
            backend: None,
            mcp_servers: Vec::new(),
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
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub context_percent: f32,
    /// Self-paced cadence hint from `observe()` (seconds until next check), set
    /// when the agent calls the `suggest_next_check` tool. `None` for `step()`.
    pub suggested_next_check_seconds: Option<u32>,
}

/// Status of the active goal (for the `/goal` status view).
pub struct GoalStatus {
    pub condition: String,
    pub elapsed_seconds: u64,
    pub turns_evaluated: u32,
    pub last_reason: Option<String>,
}

/// Result of evaluating the active goal against the conversation.
pub struct GoalEvaluation {
    pub met: bool,
    pub reason: String,
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

// ============================================================================
// Backend process wiring
// ============================================================================

/// The backend agent command to spawn, in precedence order: the
/// `VOICE_AGENT_BACKEND` env var (the runtime override), then the config's
/// `backend`, then `gallium` on `PATH`. Any of them may carry arguments
/// (`"prog arg1 arg2"`). The `app-server` argument is appended by
/// [`app_server_client::AppServerClient::spawn`].
///
/// The config has to be able to select this. Without it, a file named
/// `codex.yaml` still spawned `gallium` — which ignored the forwarded cloud
/// settings and ran its own local model, so the config looked silently ignored.
fn backend_command(config: &AgentConfig) -> (String, Vec<String>, &'static str) {
    resolve_backend(
        std::env::var("VOICE_AGENT_BACKEND").ok().as_deref(),
        config.backend.as_deref(),
    )
}

/// The pure half of [`backend_command`], so the precedence can be tested without
/// mutating process-global environment from parallel tests.
fn resolve_backend(
    env_override: Option<&str>,
    config_backend: Option<&str>,
) -> (String, Vec<String>, &'static str) {
    let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    let (spec, source) = env_override
        .and_then(non_empty)
        .map(|s| (s, "VOICE_AGENT_BACKEND"))
        .or_else(|| {
            config_backend
                .and_then(non_empty)
                .map(|s| (s, "config `backend`"))
        })
        .unwrap_or_else(|| ("gallium".to_string(), "default"));
    let mut parts = spec.split_whitespace();
    let program = parts.next().unwrap_or("gallium").to_string();
    let args: Vec<String> = parts.map(String::from).collect();
    (program, args, source)
}

/// Translate `AgentConfig` into the environment the backend's `app-server` reads
/// (the same keys `gallium-agent`/`voice-agent-cli` accept). Only present values are
/// set, so the backend's own defaults still apply.
fn backend_envs(config: &AgentConfig) -> Vec<(String, String)> {
    let mut envs = Vec::new();
    let mut set = |k: &str, v: String| envs.push((k.to_string(), v));
    if let Some(p) = &config.model_path {
        set("MODEL_PATH", p.clone());
    }
    if let Some(k) = &config.api_key {
        set("OPENAI_API_KEY", k.clone());
    }
    set("LLM_BASE_URL", config.base_url.clone());
    set("LLM_MODEL", config.model.clone());
    set("MAX_TOKENS", config.max_tokens.to_string());
    if let Some(t) = config.temperature {
        set("LLM_TEMPERATURE", t.to_string());
    }
    if let Some(r) = &config.reasoning_effort {
        set("REASONING_EFFORT", r.clone());
    }
    if let Some(e) = &config.inference_engine {
        set("INFERENCE_ENGINE", e.clone());
    }
    envs
}

/// The ambient loop's self-pacing tool, served client-side so the cadence hint
/// the backend model chooses lands back here (the in-process version shared an
/// `AtomicU64` with the tool; over the wire the client tool writes the same cell).
struct SuggestNextCheckClientTool {
    next_check: Arc<AtomicU64>,
}

impl app_server_client::ClientTool for SuggestNextCheckClientTool {
    fn name(&self) -> &str {
        "suggest_next_check"
    }
    fn description(&self) -> &str {
        "During an ambient/background check, propose how many seconds until the next \
         check — shorter when on-screen activity is changing, longer when idle."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "seconds": { "type": "integer", "description": "Seconds until the next check (clamped to 30..=3600)" },
                "reason": { "type": "string", "description": "Brief reason for the chosen interval (optional)" }
            },
            "required": ["seconds"]
        })
    }
    fn call(&self, args: Value) -> Result<String, String> {
        let seconds = args
            .get("seconds")
            .and_then(Value::as_u64)
            .ok_or_else(|| "'seconds' is required".to_string())?;
        let clamped = seconds.clamp(30, 3600);
        self.next_check.store(clamped, Ordering::SeqCst);
        Ok(format!("Next check in {clamped}s."))
    }
}

// ============================================================================
// Agent — an app-server client driving a backend agent process
// ============================================================================

/// The voice-assistant agent. It no longer runs inference in-process: it spawns a
/// backend agent (`gallium-agent app-server` by default) and drives it a turn at
/// a time over the app-server protocol, serving screen `capture`, situation, and
/// it as client tools. Goals, situation, and backchannel remain local
/// orchestration here.
pub struct Agent {
    config: AgentConfig,
    client: Arc<app_server_client::AppServerClient>,
    /// Local mirror of the conversation, for `get_conversation_history` and goal
    /// evaluation (the authoritative history lives in the backend thread).
    memory: Arc<Mutex<ConversationMemory>>,
    system_prompt: Arc<Mutex<Option<String>>>,
    skill_registry: Arc<skill::SkillRegistry>,
    situation: Arc<situation::SituationMessages>,
    capture_request_rx: Receiver<capture::CaptureRequest>,
    capture_result_tx: Sender<capture::CaptureResult>,
    find_result_tx: Sender<capture::CaptureResult>,
    ocr_result_tx: Sender<capture::CaptureResult>,
    list_result_tx: Sender<capture::CaptureResult>,
    next_check: Arc<AtomicU64>,
    goal: Mutex<Option<goal::GoalState>>,
    /// Whether the main conversation thread has been opened on the backend yet
    /// (opened lazily on the first turn so a system prompt / skills set beforehand
    /// are carried in as developer instructions).
    thread_started: Mutex<bool>,
    /// `approvalPolicy` for the **main conversation thread**. `"untrusted"` when a
    /// [`MutationApprover`] was supplied (the backend then raises approval
    /// requests, which the approver answers); `"never"` when none was (the
    /// backend runs mutations autonomously). Throwaway observe/goal threads always
    /// use `"never"` — they must not block on a prompt.
    mutation_policy: String,
}

/// How the frontend answers a mutation-approval request. The Rust mirror of the
/// UDL `ApprovalDecision` enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    Deny,
}

/// Frontend-implemented approval gate (delivered over UniFFI as a foreign trait).
/// `action` is a short verb like `"run command"` or `"file change"`; `target` is
/// the command or a human-readable description of the change. Called on a
/// backend-servicing thread and blocks the turn until it returns.
pub trait MutationApprover: Send + Sync {
    fn approve(&self, action: String, target: String) -> ApprovalDecision;
}

/// `approvalPolicy` sent for the main conversation thread when a frontend
/// supplies an approver. `"untrusted"` makes the backend run trivially-safe
/// reads (ls/cat/…) silently but escalate every mutation — the file write or
/// shell command — to the approver. See the codex `--ask-for-approval` and
/// gallium `RemoteApprovalSink` semantics.
const MUTATION_APPROVAL_POLICY: &str = "untrusted";

/// Adapts a foreign [`MutationApprover`] (implemented in the frontend, delivered
/// over UniFFI) to the internal [`app_server_client::Approver`] the client calls.
struct ApproverAdapter(Arc<dyn MutationApprover>);

impl app_server_client::Approver for ApproverAdapter {
    fn approve(&self, action: &str, target: &str) -> app_server_client::ApprovalReply {
        match self.0.approve(action.to_string(), target.to_string()) {
            ApprovalDecision::AllowOnce => app_server_client::ApprovalReply::Accept,
            ApprovalDecision::AllowSession => app_server_client::ApprovalReply::AcceptForSession,
            ApprovalDecision::Deny => app_server_client::ApprovalReply::Decline,
        }
    }
}

/// Top-level constructor. Spawns the backend agent process and negotiates the
/// connection; the conversation thread itself is opened lazily on the first turn.
pub fn agent_new(
    config: AgentConfig,
    approver: Option<Arc<dyn MutationApprover>>,
) -> Result<Arc<Agent>, AgentError> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let skill_registry = Arc::new(skill::SkillRegistry::new());
    let situation = Arc::new(situation::SituationMessages::default());
    let capture_bridge = capture::CaptureBridge::new();
    let next_check = Arc::new(AtomicU64::new(0));

    // Client tools served back to the backend: screen capture, the situation
    // reader, and the ambient pacing hint. The backend keeps its own
    // file/bash/skill tools — those run there, in the working dir we pass.
    let mut tools: Vec<Arc<dyn app_server_client::ClientTool>> = Vec::new();
    let capture_handlers: Vec<Box<dyn tool::ToolHandler>> = vec![
        Box::new(capture::CaptureScreenTool::new(
            capture_bridge.request_tx.clone(),
            capture_bridge.capture_result_rx.clone(),
        )),
        Box::new(capture::FindWindowTool::new(
            capture_bridge.request_tx.clone(),
            capture_bridge.find_result_rx.clone(),
        )),
        Box::new(capture::ApplyOcrTool::new(
            capture_bridge.request_tx.clone(),
            capture_bridge.ocr_result_rx.clone(),
        )),
        Box::new(capture::ListWindowsTool::new(
            capture_bridge.request_tx.clone(),
            capture_bridge.list_result_rx.clone(),
        )),
    ];
    for handler in capture_handlers {
        tools.push(Arc::new(app_server_client::HandlerClientTool(handler)));
    }
    tools.push(Arc::new(app_server_client::HandlerClientTool(Box::new(
        situation::ReadSituationMessagesTool::new(situation.clone()),
    ))));
    tools.push(Arc::new(SuggestNextCheckClientTool {
        next_check: next_check.clone(),
    }));

    // An approver means the frontend wants a gate: tell the backend to escalate
    // mutations (policy "untrusted"), and route those requests to the frontend.
    // No approver means run autonomously — the backend is told "never", so it
    // never raises approval requests (DeclineApprover is then never consulted).
    let (approver, mutation_policy): (Arc<dyn app_server_client::Approver>, String) = match approver
    {
        Some(a) => (
            Arc::new(ApproverAdapter(a)),
            MUTATION_APPROVAL_POLICY.to_string(),
        ),
        None => (
            Arc::new(app_server_client::DeclineApprover),
            "never".to_string(),
        ),
    };

    // Spawn and connect the backend.
    let (program, args, backend_source) = backend_command(&config);
    let envs = backend_envs(&config);
    let client =
        app_server_client::AppServerClient::spawn(&program, &args, &envs, tools, approver)?;
    let user_agent = client
        .initialize("voice-agent")
        .map_err(|e| AgentError::ConfigError(format!("backend handshake failed: {e}")))?;
    // Name where the choice came from: picking the wrong backend is otherwise
    // invisible until the model answers, and looks like the config was ignored.
    tracing::info!(
        "connected to backend '{}' from {} ({})",
        program,
        backend_source,
        user_agent
    );

    Ok(Arc::new(Agent {
        config,
        client,
        memory: Arc::new(Mutex::new(ConversationMemory::new())),
        system_prompt: Arc::new(Mutex::new(None)),
        skill_registry,
        situation,
        capture_request_rx: capture_bridge.request_rx,
        capture_result_tx: capture_bridge.capture_result_tx,
        find_result_tx: capture_bridge.find_result_tx,
        ocr_result_tx: capture_bridge.ocr_result_tx,
        list_result_tx: capture_bridge.list_result_tx,
        next_check,
        goal: Mutex::new(None),
        thread_started: Mutex::new(false),
        mutation_policy,
    }))
}

impl Agent {
    /// System prompt + skill catalog, combined into the backend thread's
    /// developer instructions.
    fn developer_instructions(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(p) = self.system_prompt.lock().clone() {
            parts.push(p);
        }
        if let Some(catalog) = self.skill_registry.catalog() {
            parts.push(catalog);
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// Build the codex-style `config` table forwarding our configured MCP servers
    /// to the backend (it connects them). `None` when none are configured.
    fn mcp_config(&self) -> Option<Value> {
        if self.config.mcp_servers.is_empty() {
            return None;
        }
        let mut servers = serde_json::Map::new();
        for (i, s) in self.config.mcp_servers.iter().enumerate() {
            let entry = match &s.url {
                Some(url) if !url.is_empty() => json!({ "url": url }),
                _ => json!({ "command": s.command, "args": s.args }),
            };
            servers.insert(format!("server_{i}"), entry);
        }
        Some(json!({ "mcp_servers": servers }))
    }

    /// Open the main conversation thread on the backend if it isn't open yet.
    fn ensure_thread(&self) -> Result<(), AgentError> {
        let mut started = self.thread_started.lock();
        if !*started {
            let instr = self.developer_instructions();
            self.client.start_thread(
                self.config.working_dir.as_deref(),
                None,
                instr.as_deref(),
                Some(&self.mutation_policy),
                self.mcp_config(),
            )?;
            *started = true;
        }
        Ok(())
    }

    fn make_response(&self, content: String, suggested: Option<u32>) -> AgentResponse {
        AgentResponse {
            content,
            role: "assistant".to_string(),
            is_final: true,
            keywords: None,
            reasoning: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            context_percent: 0.0,
            suggested_next_check_seconds: suggested,
        }
    }

    fn suggested_next_check(&self) -> Option<u32> {
        match self.next_check.load(Ordering::SeqCst) {
            0 => None,
            n => Some(n.min(u32::MAX as u64) as u32),
        }
    }

    /// Process a user input: drive one backend turn, mirror it into local memory,
    /// and return the reply.
    pub fn step(&self, user_input: String) -> Result<AgentResponse, AgentError> {
        self.next_check.store(0, Ordering::SeqCst);
        self.ensure_thread()?;

        let reply = self.client.run_turn(&user_input)?;

        let mut memory = self.memory.lock();
        memory.add_message(ChatMessage::user(user_input));
        memory.add_message(ChatMessage::assistant(reply.clone()));

        Ok(self.make_response(reply, self.suggested_next_check()))
    }

    /// Reset the conversation: clear the local mirror and open a fresh backend
    /// thread on the next turn.
    pub fn reset(&self) {
        self.memory.lock().clear();
        *self.thread_started.lock() = false;
    }

    /// Get the conversation history as JSON string (the local mirror).
    pub fn get_conversation_history(&self) -> String {
        let memory = self.memory.lock();
        serde_json::to_string_pretty(&memory.get_messages()).unwrap_or_default()
    }

    /// Set a custom system prompt. Applied as the backend thread's developer
    /// instructions when the thread (re)opens — call before the first turn, or
    /// `reset()` to apply it to a running conversation.
    pub fn set_system_prompt(&self, prompt: String) {
        *self.system_prompt.lock() = Some(prompt);
        tracing::info!("System prompt set");
    }

    /// Register a skill (its catalog is injected into the backend thread's
    /// developer instructions when the thread (re)opens).
    pub fn add_skill(&self, name: String, description: String, prompt: String) {
        self.skill_registry.add(name, description, prompt);
    }

    /// Run a one-shot, **non-persisting** turn for ambient/background observation
    /// (the `/loop` ambient mode) on a throwaway backend thread, so periodic
    /// checks don't pollute the conversation. `allowed_tools` is advisory only —
    /// the backend owns its tool set — but the pacing hint still flows back.
    pub fn observe(&self, prompt: String) -> Result<AgentResponse, AgentError> {
        self.next_check.store(0, Ordering::SeqCst);
        let instr = self.developer_instructions();
        let thread = self.client.open_thread(
            self.config.working_dir.as_deref(),
            None,
            instr.as_deref(),
            Some("never"),
            self.mcp_config(),
        )?;
        let reply = self.client.run_turn_on(&thread, &prompt)?;
        Ok(self.make_response(reply, self.suggested_next_check()))
    }

    /// Drain all pending capture requests (Swift polls this).
    pub fn drain_capture_requests(&self) -> Vec<capture::CaptureRequest> {
        let mut requests = Vec::new();
        while let Ok(req) = self.capture_request_rx.try_recv() {
            requests.push(req);
        }
        requests
    }

    /// Submit a capture result from Swift back to the waiting client tool.
    /// Routes to the correct channel based on request ID prefix.
    pub fn submit_capture_result(&self, id: String, image_base64: String, metadata_json: String) {
        let result = capture::CaptureResult {
            id: id.clone(),
            image_base64,
            metadata_json,
        };
        if id.starts_with("find_") {
            let _ = self.find_result_tx.send(result);
        } else if id.starts_with("ocr_") {
            let _ = self.ocr_result_tx.send(result);
        } else if id.starts_with("list_") {
            let _ = self.list_result_tx.send(result);
        } else {
            let _ = self.capture_result_tx.send(result);
        }
    }

    /// Push a situation message from Swift (e.g. periodic window list).
    pub fn push_situation_message(&self, text: String, source: String, session_id: String) {
        self.situation.push(text, source, session_id);
    }

    /// Set (or replace) the active goal — a completion condition the agent works
    /// toward across turns until `evaluate_goal` reports it met. Session-scoped.
    pub fn set_goal(&self, condition: String) {
        *self.goal.lock() = Some(goal::GoalState::new(condition));
        tracing::info!("Goal set");
    }

    /// Clear the active goal, if any.
    pub fn clear_goal(&self) {
        *self.goal.lock() = None;
        tracing::info!("Goal cleared");
    }

    /// Snapshot the active goal for the status view, or `None` if no goal is set.
    pub fn goal_status(&self) -> Option<GoalStatus> {
        self.goal.lock().as_ref().map(|g| GoalStatus {
            condition: g.condition.clone(),
            elapsed_seconds: g.started_at.elapsed().as_secs(),
            turns_evaluated: g.turns_evaluated,
            last_reason: g.last_reason.clone(),
        })
    }

    /// Evaluate the active goal against the recent (local-mirror) conversation on
    /// a throwaway backend thread. Records the reason, bumps the turn counter, and
    /// clears the goal when met.
    pub fn evaluate_goal(&self) -> Result<GoalEvaluation, AgentError> {
        let condition = match self.goal.lock().as_ref() {
            Some(g) => g.condition.clone(),
            None => {
                return Ok(GoalEvaluation {
                    met: false,
                    reason: "No active goal.".to_string(),
                })
            }
        };

        let recent = {
            let memory = self.memory.lock();
            memory.get_last_messages(goal::EVAL_CONTEXT_MESSAGES)
        };
        let transcript = goal::format_transcript(&recent);
        let eval_messages = goal::build_eval_messages(&condition, &transcript);
        // Flatten the [system, user] eval prompt onto one throwaway turn: the
        // system message becomes developer instructions, the user message the
        // turn input.
        let instructions = eval_messages
            .iter()
            .find(|m| matches!(m.role, ChatRole::System))
            .map(|m| m.content.clone());
        let prompt = eval_messages
            .iter()
            .find(|m| matches!(m.role, ChatRole::User))
            .map(|m| m.content.clone())
            .unwrap_or_default();

        // Goal eval is tool-less and read-only; no MCP servers needed.
        let thread =
            self.client
                .open_thread(None, None, instructions.as_deref(), Some("never"), None)?;
        let raw = self.client.run_turn_on(&thread, &prompt)?;
        let (met, reason) = goal::parse_evaluation(&raw);

        {
            let mut g = self.goal.lock();
            if let Some(state) = g.as_mut() {
                state.turns_evaluated += 1;
                state.last_reason = Some(reason.clone());
            }
            if met {
                *g = None; // achieved → clear
            }
        }

        tracing::info!("Goal evaluation: met={} reason={}", met, reason);
        Ok(GoalEvaluation { met, reason })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config naming a backend selects it. Without this, `--config
    /// configs/codex.yaml` spawned `gallium` — which ignored the forwarded cloud
    /// settings and ran its own local model, so the config looked ignored.
    #[test]
    fn config_backend_selects_the_program() {
        let (program, args, source) = resolve_backend(None, Some("codex"));
        assert_eq!(program, "codex");
        assert!(args.is_empty());
        assert_eq!(source, "config `backend`");
    }

    /// The env var is the runtime override, so it beats the config.
    #[test]
    fn env_override_beats_the_config() {
        let (program, _, source) = resolve_backend(Some("codex"), Some("gallium"));
        assert_eq!(program, "codex");
        assert_eq!(source, "VOICE_AGENT_BACKEND");
    }

    /// Neither set: `gallium` on PATH, as before.
    #[test]
    fn falls_back_to_gallium() {
        let (program, _, source) = resolve_backend(None, None);
        assert_eq!(program, "gallium");
        assert_eq!(source, "default");
    }

    /// Blank is not a selection — an unset-but-exported env var or an empty YAML
    /// value must fall through rather than try to spawn "".
    #[test]
    fn blank_values_fall_through() {
        let (program, _, source) = resolve_backend(Some("   "), Some("codex"));
        assert_eq!(program, "codex");
        assert_eq!(source, "config `backend`");

        let (program, _, source) = resolve_backend(Some(""), Some(""));
        assert_eq!(program, "gallium");
        assert_eq!(source, "default");
    }

    /// Either source may carry arguments.
    #[test]
    fn a_backend_spec_may_carry_arguments() {
        let (program, args, _) = resolve_backend(None, Some("my-agent --flag x"));
        assert_eq!(program, "my-agent");
        assert_eq!(args, vec!["--flag", "x"]);
    }
}
