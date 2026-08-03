//! App-server client: drive a whole-turn agent backend over line-delimited
//! JSON-RPC (the codex-app-server method subset — `thread/start`, `turn/start`,
//! `item/*`). This is **not** the Agent Client Protocol (ACP), which uses an
//! entirely different method set (`session/new`, `session/prompt`, `fs/*`, …).
//!
//! voice-agent spawns a backend that speaks the codex-app-server subset — `gallium
//! app-server` (the default) or `codex app-server` — and drives it a turn at a
//! time, while serving its own local tools (screen `capture`, the situation
//! reader) back to that backend as the protocol's `dynamicTools`.
//!
//! Transport is shared with the server: [`crate::appserver::rpc`] is symmetric
//! (it answers inbound requests, delivers inbound responses to our outbound
//! requests, and dispatches inbound requests on their own threads), so the same
//! `Connection` + `serve` drive either direction. Here voice-agent *sends*
//! `initialize` / `thread/start` / `turn/start` and *handles* the backend's
//! `item/tool/call` and approval requests.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};

use crate::appserver::rpc::{serve, Connection, HandlerResult, RequestHandler, RpcFault};
use crate::AgentError;

/// A tool the client serves back to the backend agent (e.g. a screen capture,
/// the situation reader). The backend's model calls it; the request arrives here as an
/// `item/tool/call` and is executed against local, resident state.
pub trait ClientTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for the tool's arguments.
    fn input_schema(&self) -> Value;
    /// `Ok(text)` on success; `Err(detail)` reports a tool failure back to the
    /// model (a normal ReAct outcome, not a transport error).
    fn call(&self, args: Value) -> Result<String, String>;
}

/// Serve an existing [`ToolHandler`] (`capture`, `read_situation_messages`, …) back to the
/// backend as a [`ClientTool`], unchanged. The tool runs in-process against its
/// resident state; only `ToolResult.text` crosses the wire (images are dropped —
/// the app-server tool-call response carries text only for now).
pub struct HandlerClientTool(pub Box<dyn crate::tool::ToolHandler>);

impl ClientTool for HandlerClientTool {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn input_schema(&self) -> Value {
        self.0.parameters_schema()
    }
    fn call(&self, args: Value) -> Result<String, String> {
        self.0.call(args).map(|r| r.text).map_err(|e| e.to_string())
    }
}

/// How the client answers a mutation-approval request the backend raises for a
/// `write`/`edit`/`bash` it wants to run in *its* process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalReply {
    Accept,
    AcceptForSession,
    Decline,
}

impl ApprovalReply {
    fn wire(self) -> &'static str {
        match self {
            ApprovalReply::Accept => "accept",
            ApprovalReply::AcceptForSession => "accept_for_session",
            ApprovalReply::Decline => "decline",
        }
    }
}

/// Decides mutation approvals raised by the backend.
pub trait Approver: Send + Sync {
    /// `action` is e.g. `"run command"` or `"file change"`; `target` is the
    /// command or a human-readable reason.
    fn approve(&self, action: &str, target: &str) -> ApprovalReply;
}

/// Declines every mutation. A safe default: a backend turn that only reads or
/// calls client tools is unaffected; a `write`/`bash` is refused rather than
/// silently granted. Frontends that want writes install their own [`Approver`].
pub struct DeclineApprover;

impl Approver for DeclineApprover {
    fn approve(&self, _action: &str, _target: &str) -> ApprovalReply {
        ApprovalReply::Decline
    }
}

/// Pull a human-readable target out of an approval-request payload, trying each
/// field name in order (the two backends name it differently), then falling back
/// to a compact JSON of the whole params so the approver is never shown a blank.
fn approval_target(params: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(s) = params.get(*key).and_then(Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    params.to_string()
}

/// How a turn ended, as reported by `turn/completed` / `turn/failed`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TurnOutcome {
    /// `turn/completed`. `status` is codex's `completed` | `interrupted` | ….
    Completed { status: String },
    /// gallium's `turn/failed`. Codex has no such notification — it spells every
    /// ending as `turn/completed` — so a client must accept both.
    Failed { message: String },
}

/// What we know about one turn. Created on demand: the backend's notifications
/// for a turn can arrive **before** the `turn/start` response that names it, so
/// the reader cannot assume the waiter registered the turn first.
#[derive(Default)]
struct TurnSlot {
    /// `agentMessage` texts in arrival order. A turn emits more than one once it
    /// can be steered, so these accumulate rather than overwrite.
    messages: Vec<String>,
    /// `None` while the turn is still running.
    outcome: Option<TurnOutcome>,
}

/// Inbound state the reader thread and the calling thread share.
struct Shared {
    tools: HashMap<String, Arc<dyn ClientTool>>,
    approver: Arc<dyn Approver>,
    /// In-flight and just-finished turns, keyed by turn id. Entries are removed
    /// by whoever waits on them.
    turns: Mutex<HashMap<String, TurnSlot>>,
    /// Woken whenever `turns` changes or the connection closes.
    turn_signal: Condvar,
    /// Set when the reader loop exits (the backend closed its stdout, or died).
    /// Waiters must give up rather than block on a turn that can never complete.
    closed: AtomicBool,
}

impl Shared {
    /// Record an `agentMessage` for `turn_id`, creating the slot if the reader
    /// got here before the waiter.
    fn push_message(&self, turn_id: &str, text: &str) {
        self.turns
            .lock()
            .entry(turn_id.to_string())
            .or_default()
            .messages
            .push(text.to_string());
        self.turn_signal.notify_all();
    }

    /// Mark `turn_id` finished and wake its waiter.
    fn finish_turn(&self, turn_id: &str, outcome: TurnOutcome) {
        self.turns
            .lock()
            .entry(turn_id.to_string())
            .or_default()
            .outcome = Some(outcome);
        self.turn_signal.notify_all();
    }

    /// Mark the connection dead and release every waiter.
    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.turn_signal.notify_all();
    }
}

/// Pull a turn id out of a payload that may name it either way: `turn.id` (the
/// current shape, used by `turn/start` responses and `turn/completed`) or a flat
/// `turnId` (`item/completed`, gallium's `turn/failed`, and older backends).
fn turn_id_of(value: &Value) -> Option<&str> {
    nested_or_flat_id(value, "turn", "turnId")
}

/// The same two-shapes problem for threads: codex answers `thread/start` with
/// `{thread:{id,…}}` (`ThreadStartResponse`), gallium with a flat `{threadId}`.
fn thread_id_of(value: &Value) -> Option<&str> {
    nested_or_flat_id(value, "thread", "threadId")
}

/// `{ <object>: { id } }`, else `{ <flat_key> }`. The two backends disagree on
/// which shape they use, and the disagreement is per-message rather than
/// per-backend, so every id read goes through this.
fn nested_or_flat_id<'a>(value: &'a Value, object: &str, flat_key: &str) -> Option<&'a str> {
    value
        .get(object)
        .and_then(|o| o.get("id"))
        .and_then(Value::as_str)
        .or_else(|| value.get(flat_key).and_then(Value::as_str))
}

/// Services the backend's inbound traffic: tool calls, approval requests, and the
/// `item/completed` notification that carries the turn's final text.
struct ClientHandler {
    shared: Arc<Shared>,
}

impl RequestHandler for ClientHandler {
    fn handle_request(
        &self,
        _conn: &Arc<Connection>,
        method: &str,
        params: Value,
    ) -> HandlerResult {
        match method {
            "item/tool/call" => {
                let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let (success, text) = match self.shared.tools.get(tool) {
                    Some(t) => match t.call(args) {
                        Ok(text) => (true, text),
                        Err(detail) => (false, detail),
                    },
                    None => (false, format!("unknown client tool '{tool}'")),
                };
                Ok(json!({
                    "success": success,
                    "contentItems": [{ "type": "inputText", "text": text }],
                }))
            }
            "item/commandExecution/requestApproval" => {
                let target = approval_target(&params, &["command", "reason"]);
                let reply = self.shared.approver.approve("run command", &target);
                Ok(json!({ "decision": reply.wire() }))
            }
            "item/fileChange/requestApproval" => {
                // gallium sends `reason`; codex describes the edit differently
                // (path/changes/description), so fall through those before giving
                // up to a compact JSON dump — the approver must always see *what*.
                let target =
                    approval_target(&params, &["reason", "path", "description", "changes"]);
                let reply = self.shared.approver.approve("file change", &target);
                Ok(json!({ "decision": reply.wire() }))
            }
            _ => Err(RpcFault::method_not_found(method)),
        }
    }

    fn handle_notification(&self, _conn: &Arc<Connection>, method: &str, params: Value) {
        match method {
            // Carries the turn's text. One turn can emit several of these.
            "item/completed" => {
                let item = params.get("item");
                let is_message = item.and_then(|i| i.get("type")).and_then(Value::as_str)
                    == Some("agentMessage");
                if !is_message {
                    return;
                }
                if let (Some(turn), Some(text)) = (
                    turn_id_of(&params),
                    item.and_then(|i| i.get("text")).and_then(Value::as_str),
                ) {
                    self.shared.push_message(turn, text);
                }
            }
            // The turn is over. This — not the `turn/start` response — is what a
            // caller waits on: both backends answer `turn/start` immediately and
            // run the turn in the background, precisely so it stays interruptible.
            "turn/completed" => {
                if let Some(turn) = turn_id_of(&params) {
                    let status = params
                        .get("turn")
                        .and_then(|t| t.get("status"))
                        .and_then(Value::as_str)
                        .unwrap_or("completed")
                        .to_string();
                    self.shared
                        .finish_turn(turn, TurnOutcome::Completed { status });
                }
            }
            // gallium only; codex folds failures into `turn/completed`.
            "turn/failed" => {
                if let Some(turn) = turn_id_of(&params) {
                    let message = params
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("turn failed")
                        .to_string();
                    self.shared
                        .finish_turn(turn, TurnOutcome::Failed { message });
                }
            }
            _ => {}
        }
    }
}

/// A driven connection to a backend agent process.
pub struct AppServerClient {
    conn: Arc<Connection>,
    shared: Arc<Shared>,
    thread_id: Mutex<Option<String>>,
    /// The backend subprocess, if we spawned one. Killed on drop. `None` for an
    /// in-process (test) transport.
    child: Mutex<Option<Child>>,
    reader: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl AppServerClient {
    /// Spawn `program args… app-server` and drive it. `envs` are set on the child
    /// (the backend reads its model/API config from the environment). `tools` are
    /// served back to the backend as `dynamicTools`; `approver` answers its
    /// mutation requests.
    pub fn spawn(
        program: &str,
        args: &[String],
        envs: &[(String, String)],
        tools: Vec<Arc<dyn ClientTool>>,
        approver: Arc<dyn Approver>,
    ) -> Result<Arc<Self>, AgentError> {
        let mut child = Command::new(program)
            .args(args)
            .arg("app-server")
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| AgentError::ConfigError(format!("spawn backend '{program}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::InternalError("backend stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::InternalError("backend stdout missing".into()))?;

        let client = Self::new_over(BufReader::new(stdout), Box::new(stdin), tools, approver);
        *client.child.lock() = Some(child);
        Ok(client)
    }

    /// Build a client over an arbitrary transport (a subprocess's pipes, or an
    /// in-memory duplex for tests). Spawns the reader thread.
    pub fn new_over<R: BufRead + Send + 'static>(
        reader: R,
        writer: Box<dyn Write + Send>,
        tools: Vec<Arc<dyn ClientTool>>,
        approver: Arc<dyn Approver>,
    ) -> Arc<Self> {
        let tool_map = tools
            .into_iter()
            .map(|t| (t.name().to_string(), t))
            .collect();
        let shared = Arc::new(Shared {
            tools: tool_map,
            approver,
            turns: Mutex::new(HashMap::new()),
            turn_signal: Condvar::new(),
            closed: AtomicBool::new(false),
        });
        let conn = Connection::new(writer);
        let handler = Arc::new(ClientHandler {
            shared: Arc::clone(&shared),
        });

        let reader_conn = Arc::clone(&conn);
        let reader_shared = Arc::clone(&shared);
        let reader_handle = std::thread::spawn(move || {
            serve(reader, reader_conn, handler);
            // `serve` returns on EOF or a read error: the backend is gone and no
            // further `turn/completed` can arrive. Release anyone waiting on a
            // turn, or they block forever.
            reader_shared.mark_closed();
        });

        Arc::new(Self {
            conn,
            shared,
            thread_id: Mutex::new(None),
            child: Mutex::new(None),
            reader: Mutex::new(Some(reader_handle)),
        })
    }

    /// Negotiate capabilities. Returns the backend's `userAgent`.
    ///
    /// `clientInfo.version` and `capabilities.requestAttestation` are **required**
    /// by codex (`ClientInfo`/`InitializeCapabilities` in its app-server protocol)
    /// — omitting them fails the handshake outright with "missing field
    /// `version`", so the codex backend could never connect. gallium ignores the
    /// extra fields, so one shape serves both.
    pub fn initialize(&self, client_name: &str) -> Result<String, AgentError> {
        let resp = self.conn.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": client_name,
                    "title": Value::Null,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                    "requestAttestation": false,
                },
            }),
        )?;
        Ok(resp
            .get("userAgent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string())
    }

    /// Open a thread, registering our [`ClientTool`]s as the backend's
    /// `dynamicTools`, and return its id. Does **not** become the client's main
    /// thread — use [`start_thread`](Self::start_thread) for that. Handy for a
    /// throwaway thread (an ambient observation, a goal evaluation) that must not
    /// pollute the conversation history.
    pub fn open_thread(
        &self,
        cwd: Option<&str>,
        model: Option<&str>,
        developer_instructions: Option<&str>,
        approval_policy: Option<&str>,
        config: Option<Value>,
        skill_paths: &[String],
    ) -> Result<String, AgentError> {
        let dynamic_tools: Vec<Value> = self
            .shared
            .tools
            .values()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name(),
                    "description": t.description(),
                    "inputSchema": t.input_schema(),
                })
            })
            .collect();

        let mut params = json!({ "dynamicTools": dynamic_tools });
        if let Some(cwd) = cwd {
            params["cwd"] = json!(cwd);
        }
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        if let Some(instr) = developer_instructions {
            params["developerInstructions"] = json!(instr);
        }
        if let Some(policy) = approval_policy {
            params["approvalPolicy"] = json!(policy);
        }
        // Our skills live wherever this repo puts them, which is none of the
        // directories a backend searches on its own. Naming them here is what
        // lets its skill-lookup tool agree with the catalog we inline — before
        // this, that tool answered "none" and a model that consulted it decided
        // it had no skills at all.
        //
        // A backend that does not know the field ignores it, so this is safe to
        // send to any of them.
        if !skill_paths.is_empty() {
            params["skillPaths"] = json!(skill_paths);
        }
        // codex nests MCP config under a free-form `config` table; the backend's
        // thread/start reads `config.mcp_servers` and connects them.
        if let Some(config) = config {
            params["config"] = config;
        }

        let resp = self.conn.request("thread/start", params)?;

        // gallium answers with `skillCount`; codex does not. Report what came
        // back so a path that landed nowhere is visible at thread start rather
        // than as a model later claiming it has no skills.
        if !skill_paths.is_empty() {
            match resp.get("skillCount").and_then(Value::as_u64) {
                Some(0) => tracing::warn!(
                    "backend registered no skills from skillPaths {:?} — its skill-lookup tool \
                     will report none",
                    skill_paths
                ),
                Some(n) => tracing::info!("backend registered {} skill(s)", n),
                // Older backend, or codex: it ignored the field. The inlined
                // catalog still carries the skills, so this is not a failure.
                None => tracing::debug!(
                    "backend did not report skillCount; skillPaths may be unsupported"
                ),
            }
        }

        thread_id_of(&resp).map(str::to_string).ok_or_else(|| {
            AgentError::InternalError(format!(
                "thread/start named no thread (expected `thread.id` or `threadId`): {resp}"
            ))
        })
    }

    /// Open a thread and adopt it as the client's main conversation thread (the
    /// one [`run_turn`](Self::run_turn) drives). Calling again replaces it — the
    /// way to `reset` a conversation.
    pub fn start_thread(
        &self,
        cwd: Option<&str>,
        model: Option<&str>,
        developer_instructions: Option<&str>,
        approval_policy: Option<&str>,
        config: Option<Value>,
        skill_paths: &[String],
    ) -> Result<String, AgentError> {
        let thread_id = self.open_thread(
            cwd,
            model,
            developer_instructions,
            approval_policy,
            config,
            skill_paths,
        )?;
        *self.thread_id.lock() = Some(thread_id.clone());
        Ok(thread_id)
    }

    /// Run one turn on an explicit thread and return the agent's final text.
    /// Blocks until the turn completes (the backend may call our tools
    /// reentrantly meanwhile).
    pub fn run_turn_on(&self, thread_id: &str, text: &str) -> Result<String, AgentError> {
        let resp = self.conn.request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": text }],
            }),
        )?;

        // `turn/start` is answered as soon as the turn is *accepted*, not when it
        // finishes — that is what keeps a turn interruptible. The id it names is
        // how we then follow the turn to completion.
        let turn_id = turn_id_of(&resp)
            .ok_or_else(|| {
                AgentError::InternalError(format!(
                    "turn/start response named no turn (expected `turn.id` or `turnId`): {resp}"
                ))
            })?
            .to_string();

        self.await_turn(&turn_id)
    }

    /// Block until `turn_id` reports `turn/completed` / `turn/failed`, then take
    /// its text. Notifications for the turn may already have arrived before this
    /// is called — the slot is created on demand, so nothing is lost either way.
    fn await_turn(&self, turn_id: &str) -> Result<String, AgentError> {
        let mut turns = self.shared.turns.lock();
        loop {
            // Created on demand: the reader may not have seen this turn at all yet.
            let slot = turns.entry(turn_id.to_string()).or_default();
            if slot.outcome.is_some() {
                let slot = turns.remove(turn_id).unwrap_or_default();
                return match slot.outcome {
                    Some(TurnOutcome::Failed { message }) => Err(AgentError::InternalError(
                        format!("turn '{turn_id}' failed: {message}"),
                    )),
                    // `interrupted` is a normal ending, not an error: the caller
                    // keeps whatever the turn produced before it was stopped.
                    _ => Ok(slot.messages.join("\n")),
                };
            }

            if self.shared.closed.load(Ordering::SeqCst) {
                turns.remove(turn_id);
                return Err(AgentError::InternalError(format!(
                    "backend closed the connection while turn '{turn_id}' was running"
                )));
            }

            // Timed wait rather than a bare one: a backend that never reports the
            // turn would otherwise wedge the caller forever with no way out. The
            // interval only bounds how fast we notice `closed`, not turn latency —
            // turns legitimately run for minutes.
            self.shared
                .turn_signal
                .wait_for(&mut turns, Duration::from_millis(500));
        }
    }

    /// Run one turn on the client's main conversation thread.
    pub fn run_turn(&self, text: &str) -> Result<String, AgentError> {
        let thread_id = self
            .thread_id
            .lock()
            .clone()
            .ok_or_else(|| AgentError::InternalError("run_turn before start_thread".into()))?;
        self.run_turn_on(&thread_id, text)
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        // Kill the child first: that closes its stdout, so the reader's `serve`
        // loop sees EOF and returns — only then is it safe to join.
        let had_child = if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
            true
        } else {
            false
        };
        if let Some(handle) = self.reader.lock().take() {
            // With no child (an in-process transport, e.g. tests) nothing closes
            // the reader's input, and joining while we still hold the connection's
            // writer would deadlock — the writer keeps the peer, and thus the
            // reader's input, alive. Detach instead; teardown reaps the thread.
            if had_child {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crossbeam::channel::{unbounded, Receiver, Sender};

    // --- in-memory duplex byte plumbing (mirrors appserver::e2e_tests) --------

    struct ChannelReader {
        rx: Receiver<Vec<u8>>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Read for ChannelReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            while self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(bytes) => {
                        self.buf = bytes;
                        self.pos = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = std::cmp::min(out.len(), self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    struct ByteChannelWriter {
        tx: Sender<Vec<u8>>,
    }

    impl Write for ByteChannelWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            // A dropped receiver just means the peer went away; report it as EOF-ish.
            let _ = self.tx.send(data.to_vec());
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn reader_over(rx: Receiver<Vec<u8>>) -> BufReader<ChannelReader> {
        BufReader::new(ChannelReader {
            rx,
            buf: Vec::new(),
            pos: 0,
        })
    }

    // --- a hand-rolled backend that speaks the protocol directly --------------

    /// A minimal codex-app-server peer, matching what `gallium app-server` and
    /// `codex app-server` actually do: `turn/start` is **answered immediately**
    /// with `{turn:{id,status:"inProgress"}}` and the turn then runs in the
    /// background, reporting through `item/completed` + `turn/completed`.
    ///
    /// Answering first and notifying afterwards is the whole point of the
    /// fixture: the previous stub emitted `item/completed` *before* its response
    /// and returned a flat `turnId`, a contract no shipping backend honours — so
    /// it certified a client that returned an empty reply against both real
    /// backends. See docs/STEERING.md.
    struct StubBackend;

    impl RequestHandler for StubBackend {
        fn handle_request(
            &self,
            conn: &Arc<Connection>,
            method: &str,
            _params: Value,
        ) -> HandlerResult {
            match method {
                "initialize" => Ok(json!({ "userAgent": "voice-agent-test/0.1.0" })),
                "thread/start" => Ok(json!({ "threadId": "t1" })),
                "turn/start" => {
                    // Reentrantly call the client's `ping` tool mid-turn — the
                    // reader must stay live to deliver the response.
                    let _ =
                        conn.request("item/tool/call", json!({ "tool": "ping", "arguments": {} }));

                    // The turn's result lands *after* this response, from another
                    // thread, as the real backends do.
                    let conn = Arc::clone(conn);
                    std::thread::spawn(move || {
                        conn.notify(
                            "item/completed",
                            json!({
                                "threadId": "t1",
                                "turnId": "turn1",
                                "item": { "type": "agentMessage", "text": "all done" },
                            }),
                        );
                        conn.notify(
                            "turn/completed",
                            json!({
                                "threadId": "t1",
                                "turn": { "id": "turn1", "status": "completed" },
                            }),
                        );
                    });
                    Ok(json!({ "turn": { "id": "turn1", "status": "inProgress" } }))
                }
                _ => Err(RpcFault::method_not_found(method)),
            }
        }
    }

    struct PingTool {
        called: Arc<AtomicBool>,
    }

    impl ClientTool for PingTool {
        fn name(&self) -> &str {
            "ping"
        }
        fn description(&self) -> &str {
            "returns pong"
        }
        fn input_schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        fn call(&self, _args: Value) -> Result<String, String> {
            self.called.store(true, Ordering::SeqCst);
            Ok("pong".to_string())
        }
    }

    /// A full turn where the backend calls a client tool, then answers — driven
    /// end-to-end over the shared transport, no model or network.
    #[test]
    fn drives_a_turn_that_calls_a_client_tool() {
        // client → server bytes, and server → client bytes.
        let (client_out, server_in) = unbounded::<Vec<u8>>();
        let (server_out, client_in) = unbounded::<Vec<u8>>();

        let server_conn = Connection::new(Box::new(ByteChannelWriter { tx: server_out }));
        std::thread::spawn(move || {
            serve(reader_over(server_in), server_conn, Arc::new(StubBackend));
        });

        let called = Arc::new(AtomicBool::new(false));
        let client = AppServerClient::new_over(
            reader_over(client_in),
            Box::new(ByteChannelWriter { tx: client_out }),
            vec![Arc::new(PingTool {
                called: Arc::clone(&called),
            })],
            Arc::new(DeclineApprover),
        );

        // Watchdog: a transport deadlock would otherwise hang forever. Run the
        // driving calls on a thread and fail if they don't finish promptly.
        let (done_tx, done_rx) = unbounded::<(String, bool)>();
        std::thread::spawn(move || {
            let ua = client.initialize("voice-agent-test").expect("initialize");
            assert!(ua.contains("voice-agent"), "userAgent: {ua}");
            client
                .start_thread(None, None, None, Some("never"), None, &[])
                .expect("thread/start");
            let reply = client.run_turn("hi").expect("run_turn");
            let _ = done_tx.send((reply, called.load(Ordering::SeqCst)));
        });

        let (reply, tool_called) = done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("turn completed within 10s (deadlock otherwise)");
        assert_eq!(reply, "all done");
        assert!(tool_called, "backend never called the client tool");
    }

    // --- turn-completion plumbing (see docs/STEERING.md, Stage 0) ------------

    /// Build a client wired to `backend` over an in-memory duplex.
    fn client_against(backend: Arc<dyn RequestHandler>) -> Arc<AppServerClient> {
        let (client_out, server_in) = unbounded::<Vec<u8>>();
        let (server_out, client_in) = unbounded::<Vec<u8>>();
        let server_conn = Connection::new(Box::new(ByteChannelWriter { tx: server_out }));
        std::thread::spawn(move || serve(reader_over(server_in), server_conn, backend));
        AppServerClient::new_over(
            reader_over(client_in),
            Box::new(ByteChannelWriter { tx: client_out }),
            vec![],
            Arc::new(DeclineApprover),
        )
    }

    /// Run `f` on its own thread and fail rather than hang if it wedges.
    fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = unbounded::<T>();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(secs))
            .expect("completed in time (would otherwise hang)")
    }

    /// Emits `count` agentMessages, then ends the turn with `ending`.
    struct ScriptedTurnBackend {
        count: usize,
        ending: &'static str,
    }

    impl RequestHandler for ScriptedTurnBackend {
        fn handle_request(
            &self,
            conn: &Arc<Connection>,
            method: &str,
            _params: Value,
        ) -> HandlerResult {
            match method {
                "initialize" => Ok(json!({ "userAgent": "voice-agent-test/0.1.0" })),
                "thread/start" => Ok(json!({ "threadId": "t1" })),
                "turn/start" => {
                    let conn = Arc::clone(conn);
                    let (count, ending) = (self.count, self.ending);
                    std::thread::spawn(move || {
                        for i in 0..count {
                            conn.notify(
                                "item/completed",
                                json!({
                                    "turnId": "turn1",
                                    "item": { "type": "agentMessage", "text": format!("part{i}") },
                                }),
                            );
                        }
                        match ending {
                            "failed" => conn.notify(
                                "turn/failed",
                                json!({
                                    "threadId": "t1",
                                    "turnId": "turn1",
                                    "error": { "message": "model exploded" },
                                }),
                            ),
                            status => conn.notify(
                                "turn/completed",
                                json!({
                                    "threadId": "t1",
                                    "turn": { "id": "turn1", "status": status },
                                }),
                            ),
                        }
                    });
                    Ok(json!({ "turn": { "id": "turn1", "status": "inProgress" } }))
                }
                _ => Err(RpcFault::method_not_found(method)),
            }
        }
    }

    /// The turn id lives at `turn.id`, and the reply is whatever arrived before
    /// `turn/completed` — *not* whatever happened to be present when `turn/start`
    /// was answered. Both real backends answer that request immediately, so a
    /// client that read the response as "done" returned an empty string.
    #[test]
    fn waits_for_turn_completed_rather_than_the_turn_start_response() {
        let client = client_against(Arc::new(ScriptedTurnBackend {
            count: 1,
            ending: "completed",
        }));
        let reply = within(10, move || {
            client.initialize("t").expect("initialize");
            client
                .start_thread(None, None, None, Some("never"), None, &[])
                .expect("thread/start");
            client.run_turn("hi").expect("run_turn")
        });
        assert_eq!(reply, "part0");
    }

    /// A turn can emit several agentMessages — a steered turn always will. They
    /// accumulate instead of the last one silently winning.
    #[test]
    fn collects_every_agent_message_in_a_turn() {
        let client = client_against(Arc::new(ScriptedTurnBackend {
            count: 3,
            ending: "completed",
        }));
        let reply = within(10, move || {
            client.initialize("t").expect("initialize");
            client
                .start_thread(None, None, None, Some("never"), None, &[])
                .expect("thread/start");
            client.run_turn("hi").expect("run_turn")
        });
        assert_eq!(reply, "part0\npart1\npart2");
    }

    /// gallium reports a failure as `turn/failed`; codex folds it into
    /// `turn/completed`. The client must surface the former as an error rather
    /// than hang waiting for a `turn/completed` that never comes.
    #[test]
    fn surfaces_turn_failed_as_an_error() {
        let client = client_against(Arc::new(ScriptedTurnBackend {
            count: 0,
            ending: "failed",
        }));
        let err = within(10, move || {
            client.initialize("t").expect("initialize");
            client
                .start_thread(None, None, None, Some("never"), None, &[])
                .expect("thread/start");
            client.run_turn("hi").unwrap_err().to_string()
        });
        assert!(err.contains("model exploded"), "unexpected error: {err}");
    }

    /// An interrupted turn is a normal ending: the caller keeps whatever the turn
    /// produced before it was stopped.
    #[test]
    fn treats_an_interrupted_turn_as_a_normal_ending() {
        let client = client_against(Arc::new(ScriptedTurnBackend {
            count: 1,
            ending: "interrupted",
        }));
        let reply = within(10, move || {
            client.initialize("t").expect("initialize");
            client
                .start_thread(None, None, None, Some("never"), None, &[])
                .expect("thread/start");
            client.run_turn("hi").expect("interrupted is not an error")
        });
        assert_eq!(reply, "part0");
    }

    /// A backend that accepts the turn and then dies must not wedge the caller
    /// forever: the reader hitting EOF releases the waiter.
    #[test]
    fn a_dead_backend_releases_the_turn_waiter() {
        let (client_out, server_in) = unbounded::<Vec<u8>>();
        let (server_out, client_in) = unbounded::<Vec<u8>>();

        // Hand-rolled rather than `serve`: this backend has to answer `turn/start`
        // and *then* drop its write half while the client is still waiting, which
        // is the whole scenario. Driving it through a handler cannot express that
        // — the handler's return is what keeps the connection alive.
        std::thread::spawn(move || {
            let mut lines = reader_over(server_in).lines();
            while let Some(Ok(line)) = lines.next() {
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let method = msg
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let result = match method.as_str() {
                    "initialize" => json!({ "userAgent": "voice-agent-test/0.1.0" }),
                    "thread/start" => json!({ "threadId": "t1" }),
                    "turn/start" => json!({ "turn": { "id": "turn1" } }),
                    _ => continue,
                };
                let resp = json!({ "jsonrpc": "2.0", "id": msg.get("id"), "result": result });
                let _ = server_out.send(format!("{resp}\n").into_bytes());
                if method == "turn/start" {
                    break; // accepted the turn, now die without ever reporting it
                }
            }
            drop(server_out);
        });

        let client = AppServerClient::new_over(
            reader_over(client_in),
            Box::new(ByteChannelWriter { tx: client_out }),
            vec![],
            Arc::new(DeclineApprover),
        );

        let err = within(10, move || {
            client.initialize("t").expect("initialize");
            client
                .start_thread(None, None, None, Some("never"), None, &[])
                .expect("thread/start");
            client.run_turn("hi").unwrap_err().to_string()
        });
        assert!(
            err.contains("closed the connection"),
            "unexpected error: {err}"
        );
    }

    /// A backend that, mid-turn, asks the client to approve a file change and
    /// records the `decision` string it gets back — so a test can assert the
    /// Approver's reply reached the wire.
    struct ApprovalBackend {
        seen_decision: Arc<Mutex<Option<String>>>,
    }

    impl RequestHandler for ApprovalBackend {
        fn handle_request(
            &self,
            conn: &Arc<Connection>,
            method: &str,
            _params: Value,
        ) -> HandlerResult {
            match method {
                "initialize" => Ok(json!({ "userAgent": "voice-agent-test/0.1.0" })),
                "thread/start" => Ok(json!({ "threadId": "t1" })),
                "turn/start" => {
                    let resp = conn
                        .request(
                            "item/fileChange/requestApproval",
                            json!({ "reason": "write file 'notes.md'" }),
                        )
                        .expect("approval round-trips");
                    *self.seen_decision.lock() = resp
                        .get("decision")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let conn = Arc::clone(conn);
                    std::thread::spawn(move || {
                        conn.notify(
                            "item/completed",
                            json!({
                                "threadId": "t1",
                                "turnId": "turn1",
                                "item": { "type": "agentMessage", "text": "ok" },
                            }),
                        );
                        conn.notify(
                            "turn/completed",
                            json!({
                                "threadId": "t1",
                                "turn": { "id": "turn1", "status": "completed" },
                            }),
                        );
                    });
                    Ok(json!({ "turn": { "id": "turn1", "status": "inProgress" } }))
                }
                _ => Err(RpcFault::method_not_found(method)),
            }
        }
    }

    /// Records what the backend asked us to approve, and answers `AllowSession`.
    struct RecordingApprover {
        asked: Arc<Mutex<Option<(String, String)>>>,
    }

    impl Approver for RecordingApprover {
        fn approve(&self, action: &str, target: &str) -> ApprovalReply {
            *self.asked.lock() = Some((action.to_string(), target.to_string()));
            ApprovalReply::AcceptForSession
        }
    }

    /// A backend approval request reaches the [`Approver`], and its reply is sent
    /// back to the backend as the right wire `decision`.
    #[test]
    fn routes_a_mutation_approval_to_the_approver() {
        let (client_out, server_in) = unbounded::<Vec<u8>>();
        let (server_out, client_in) = unbounded::<Vec<u8>>();

        let seen_decision = Arc::new(Mutex::new(None));
        let server_conn = Connection::new(Box::new(ByteChannelWriter { tx: server_out }));
        {
            let seen_decision = Arc::clone(&seen_decision);
            std::thread::spawn(move || {
                serve(
                    reader_over(server_in),
                    server_conn,
                    Arc::new(ApprovalBackend { seen_decision }),
                );
            });
        }

        let asked = Arc::new(Mutex::new(None));
        let client = AppServerClient::new_over(
            reader_over(client_in),
            Box::new(ByteChannelWriter { tx: client_out }),
            vec![],
            Arc::new(RecordingApprover {
                asked: Arc::clone(&asked),
            }),
        );

        let (done_tx, done_rx) = unbounded::<()>();
        std::thread::spawn(move || {
            client.initialize("voice-agent-test").expect("initialize");
            client
                .start_thread(None, None, None, Some("untrusted"), None, &[])
                .expect("thread/start");
            client.run_turn("tidy the notes").expect("run_turn");
            let _ = done_tx.send(());
        });

        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("turn completed within 10s (deadlock otherwise)");

        let asked = asked.lock().clone();
        assert_eq!(
            asked,
            Some((
                "file change".to_string(),
                "write file 'notes.md'".to_string()
            )),
            "approver saw the action + target"
        );
        assert_eq!(
            seen_decision.lock().as_deref(),
            Some("accept_for_session"),
            "AllowSession must reach the backend as accept_for_session"
        );
    }

    #[test]
    fn approval_target_prefers_named_fields_then_falls_back_to_json() {
        let p = json!({ "reason": "edit main.lua" });
        assert_eq!(approval_target(&p, &["reason", "path"]), "edit main.lua");

        // First key empty/absent → try the next.
        let p = json!({ "reason": "", "path": "/tmp/x" });
        assert_eq!(approval_target(&p, &["reason", "path"]), "/tmp/x");

        // No known field → the approver still sees *something* (the raw params).
        let p = json!({ "weird": 1 });
        let out = approval_target(&p, &["reason", "path"]);
        assert!(out.contains("weird"), "fell back to JSON: {out}");
    }

    /// A real resident tool serves verbatim through the adapter — no rewrite.
    #[test]
    fn wraps_a_resident_tool_as_a_client_tool() {
        let situation = std::sync::Arc::new(crate::situation::SituationMessages::default());
        let handler: Box<dyn crate::tool::ToolHandler> =
            Box::new(crate::situation::ReadSituationMessagesTool::new(situation));
        let tool = HandlerClientTool(handler);

        assert_eq!(tool.name(), "read_situation_messages");
        assert_eq!(tool.input_schema()["type"], "object");
        let out = tool.call(json!({})).expect("read_situation_messages runs");
        assert!(!out.is_empty(), "the tool should report something");
    }
}
