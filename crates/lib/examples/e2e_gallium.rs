//! Drives a **real** backend end-to-end through [`AppServerClient`].
//!
//! Not a test: it needs `gallium` (or another codex-app-server binary) on PATH
//! plus a model, and one turn costs real inference time. It exists because the
//! unit tests can only check the client against a stub, and the bug that
//! motivated Stage 0 of docs/STEERING.md was precisely a stub that had drifted
//! from what the backends do — it asserted the old synchronous contract and so
//! certified a client that returned an empty string against every real backend.
//!
//! ```bash
//! cd crates && cargo run --example e2e_gallium
//! # or against another backend:
//! cd crates && VOICE_AGENT_BACKEND=codex cargo run --example e2e_gallium
//! ```
//!
//! Passes when the reply is non-empty and arrives *after* the turn actually ran.

use std::sync::Arc;
use std::time::Instant;

use voice_agent_core::app_server_client::{AppServerClient, DeclineApprover};

fn main() {
    let spec = std::env::var("VOICE_AGENT_BACKEND").unwrap_or_else(|_| "gallium".to_string());
    let mut parts = spec.split_whitespace();
    let program = parts.next().unwrap_or("gallium").to_string();
    let args: Vec<String> = parts.map(String::from).collect();

    let client = AppServerClient::spawn(&program, &args, &[], vec![], Arc::new(DeclineApprover))
        .unwrap_or_else(|e| panic!("spawn backend '{program}': {e}"));

    let user_agent = client.initialize("voice-agent-e2e").expect("initialize");
    println!("backend:   {user_agent}");

    client
        .start_thread(Some("/tmp"), None, None, Some("never"), None)
        .expect("thread/start");

    let started = Instant::now();
    let reply = client
        .run_turn("say hi in exactly three words")
        .expect("run_turn");
    let elapsed = started.elapsed();

    println!("turn took: {elapsed:?}");
    println!("reply:     {reply:?}");

    assert!(
        !reply.is_empty(),
        "empty reply — the client answered before the turn finished (the Stage 0 bug)"
    );
    println!("OK");
}
