# voice-agent - Developer Guide

## Overview

A macOS/Windows voice assistant frontend. voice-agent does **no LLM inference of
its own** — it is an **ACP client** that spawns a backend agent (`gallium` by
default; `codex` via `VOICE_AGENT_ACP_BACKEND`) and drives it a turn at a time over
JSON-RPC, serving its resident tools (screen capture, situation) back to the
backend as `dynamicTools`.

- **Platform**: macOS 26+ (requires Apple SpeechTranscriber); Windows via the C# frontend
- **Swift**: swift-tools-version 6.1, `.swiftLanguageMode(.v5)` on all targets
- **Rust**: workspace in `crates/` with a single member, `lib` (the `voice_agent_core` cdylib)

## Architecture

```
Mic -> AVAudioEngine -> SpeechAnalyzer/SpeechTranscriber (STT)
    -> Swift CLI (main.swift)
    -> UniFFI bridge
    -> Rust Agent (lib.rs) — an ACP client
    -> spawns + drives  ==>  gallium app-server (the backend agent:
                              ReAct loop, LLM providers, tools, MCP)
    <-  item/tool/call   <==  backend calls voice-agent's client tools (capture, situation)
    -> final turn text
    -> AVSpeechSynthesizer (TTS) -> Speaker
```

The backend is swappable: `gallium` and `codex` both speak the same
codex-app-server JSON-RPC subset. See **[docs/REFACTOR.md](docs/REFACTOR.md)** for
the split (voice-agent = platform + ACP client; the agent core lives in
`../rs-gallium`).

### Rust Crate (`crates/lib`, `voice_agent_core`)

| File | Purpose |
|------|---------|
| `lib/src/lib.rs` | `Agent` struct + UniFFI exports. Spawns the backend, forwards config as env, serves client tools, drives turns. Goals, situation, backchannel, and the conversation mirror stay local. |
| `lib/src/acp_client.rs` | ACP client: spawns `gallium app-server` (etc.) and drives it over line-delimited JSON-RPC, reusing the symmetric `appserver::rpc` transport. Sends `initialize`/`thread/start`/`turn/start`; handles inbound `item/tool/call` + approval requests. `ClientTool`/`HandlerClientTool` wrap any `ToolHandler` to serve it back to the backend. |
| `lib/src/appserver/rpc.rs` | Symmetric JSON-RPC 2.0 transport over stdio (answers inbound requests on their own threads, delivers inbound responses to outbound requests). Shared by the ACP client. |
| `lib/src/appserver/mod.rs` | Just re-exports `rpc` now (the in-process server was removed with the agent core). |
| `lib/src/llm.rs` | Shared data types only: `ChatMessage`, `ChatRole`, `TokenUsage`, `ImageContent`, `ToolDefinition`, `ToolCallInfo`. No provider layer. |
| `lib/src/mcp.rs` | JSON-RPC 2.0 / MCP wire-type constants used by `rpc.rs`. |
| `lib/src/tool.rs` | The tool trait surface the capture/situation client tools implement: `ToolHandler`, `ToolResult`, `ToolRegistry`, `ToolAccess`. (The built-in file/bash tools and their permission machinery were removed — the backend owns those now.) |
| `lib/src/capture.rs` | Screen capture / find-window / OCR / list-windows tools (executed macOS-side via Swift; served to the backend as client tools). |
| `lib/src/situation.rs` | `SituationMessages` ambient-context stack + `read_situation_messages` client tool. Fed by the frontend's periodic window-list poller (`push_situation_message`). |
| `lib/src/goal.rs` | Session goal state + evaluation (runs on a throwaway backend thread). |
| `lib/src/skill.rs` | `SkillRegistry`; skill catalogs are injected into the backend thread's developer instructions. |
| `lib/src/memory.rs` | `ConversationMemory` — the local mirror of the conversation (authoritative history lives in the backend thread). |
| `lib/src/state_updater.rs` | Rule-based backchannel detection. |
| `lib/src/agent.udl` | UniFFI interface definition. |

### Swift Packages (`swift/Sources/`)

| Package | Purpose |
|---------|---------|
| `VoiceAgentCli` | Main entry point (text/voice REPL), window-list + capture pollers |
| `AgentKit` | `AgentSession` — shared agent lifecycle (init, skills, TTS) usable from CLI/iOS |
| `Audio` | AudioCapture (mic -> SpeechTranscriber), VoiceProcessingIO |
| `TTS` | AVSpeechSynthesizer wrapper |
| `ScreenCapture` | WindowManager / window info for the capture client tools |
| `Util` | Config, Logger, HarmonyParser, SkillLoader |
| `AgentBridge` | Generated UniFFI Swift bindings |
| `AgentBridgeFFI` | C module map for FFI |

### Key Patterns

- **voice-agent runs no inference.** `agent_new` spawns the backend (`backend_command()` — `gallium` by default, override with `VOICE_AGENT_ACP_BACKEND`), forwards model/API config as environment (`MODEL_PATH`, `OPENAI_API_KEY`, `LLM_BASE_URL`, `LLM_MODEL`, `INFERENCE_ENGINE`, …), and drives turns. `step`/`observe`/`evaluate_goal` each run a backend turn; `observe`/`evaluate_goal` use throwaway threads so they don't pollute history.
- **Client tools** (`acp_client::ClientTool`): screen `capture`, `read_situation_messages`, and `suggest_next_check` are registered as the backend's `dynamicTools`. The backend's model calls them; the request arrives as an inbound `item/tool/call` and executes against resident voice-agent state. `HandlerClientTool` adapts any `ToolHandler` verbatim.
- `ChatMessage` has `#[serde(skip)]` fields for tool state; use helper methods (`ChatMessage::user()`, `ChatMessage::assistant()`, etc.) not struct literals.
- The transport (`appserver::rpc`) is **bidirectional** — inbound requests are dispatched on their own threads so a long `turn/start` can originate tool-call requests while the reader keeps running.
- **Approvals**: `agent_new` takes an optional `MutationApprover` (a UniFFI foreign trait). When one is supplied, the main conversation thread opens with `approvalPolicy: "untrusted"` so the backend escalates every file write / shell command; the request arrives as an inbound `item/{fileChange,commandExecution}/requestApproval` and is routed to the approver, which blocks the turn until it answers allow-once / allow-session / deny. The macOS CLI's `ReplApprover` prompts on stdin in text mode, denies in voice mode (no safe way to confirm by speech), and auto-allows for one-shot `--prompt`. **With no approver the main thread opens `"never"` and the backend runs mutations autonomously** (this is what Windows does today, and it's why unconfigured builds edit files without asking). Throwaway `observe`/`evaluate_goal` threads always use `"never"` — they must not block on a prompt. voice-agent has no sandbox.
- Half-duplex: `AudioCapture.mute()`/`unmute()` drops audio buffers during TTS playback.

## Configuration

YAML configs in `configs/`. Two are shipped, one per backend flavor; the system
prompt supports the `{language}` template variable.

| config | backend | notes |
|--------|---------|-------|
| `gallium.yaml` | `gallium` (default) | local model via the standalone pure-Rust agent; `modelPath` + `inferenceEngine` forwarded as env |
| `codex.yaml` | `codex` (cloud) | set `VOICE_AGENT_ACP_BACKEND=codex` + `OPENAI_API_KEY`; `baseURL`/`model` forwarded |

```yaml
llm:
  modelPath: "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf"  # forwarded as MODEL_PATH (auto-downloaded by the backend)
  baseURL: "https://api.openai.com/v1"  # forwarded as LLM_BASE_URL (cloud)
  model: "gpt-5.6-luna"                 # forwarded as LLM_MODEL
  apiKey: ""                            # or OPENAI_API_KEY env var
  inferenceEngine: "gallium"            # forwarded as INFERENCE_ENGINE (backend's local engine: llamacpp | gallium)
  temperature: 0.7
  maxTokens: 2048
  reasoningEffort: "medium"

agent:
  systemPromptPath: "system-prompt.md"  # relative to config dir; carried into the backend thread as developer instructions
  maxTurns: 50
  language: "en"                        # "en" or "ja"

tts:  { enabled: true, voice: "com.apple.voice.enhanced.en-US.Zoe", rate: 0.5, pitchMultiplier: 1.0, volume: 1.0 }
stt:  { enabled: true, locale: "en-US", censor: false }
```

The `llm:` block is **forwarded to the backend as environment** — voice-agent does not
interpret it beyond that. Backend selection is via `VOICE_AGENT_ACP_BACKEND` (env), not
the config.

## Skills

Skills are `SKILL.md` files with YAML frontmatter loaded from:
1. `skills/` directory (relative to config file's parent)
2. `~/.claude/plugins/` (recursive)

A skill's catalog is injected into the backend thread's developer instructions.

## Build & Run

```bash
# Rust core (cdylib for the frontends)
cd crates && cargo build --release
cd crates && cargo test

# UniFFI (after .udl changes)
bash scripts/gen_uniffi.sh          # builds release + regenerates + copies into swift/Sources/AgentBridge

# Swift
cd swift && swift build

# Run (needs a backend on PATH — install `gallium` from ../rs-gallium)
cd swift && swift run voice-agent-cli --config ../configs/gallium.yaml           # local backend
VOICE_AGENT_ACP_BACKEND=codex OPENAI_API_KEY=sk-... \
  swift run voice-agent-cli --config ../configs/codex.yaml --text                # cloud backend
```

### `make install` — one binary

`make install` builds and installs the Swift voice app as **`voice-agent`** into
`$PREFIX/bin` (default `~/bin`). It links `libvoice_agent_core.dylib` by **absolute
path** into this repo's `crates/target/release`, so the repo must stay put.

The **agent backend is a separate binary** (`gallium`, built and installed from
`../rs-gallium`) found on PATH at runtime — voice-agent spawns `gallium app-server`.

## Windows CLI (`win/`)

A C# console frontend (text/listen REPL) that consumes the `voice_agent_core` cdylib
through **UniFFI C# bindings**. It produces `voice-agent.exe`, which needs
`uniffi_voice_agent_core.dll` beside it (the csproj copies the cdylib under that name).
Because voice-agent no longer does in-process inference, the cdylib has **no C++ deps
and no feature flags** — an ordinary `cargo build` with any toolchain.

```bash
# 1. Build the cdylib (voice_agent_core.dll)
scripts/build-win-local.bat
#    -> crates/target/release/voice_agent_core.dll

# 2. Generate C# bindings into win/vendor/ (install once:
#    cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.9.0+v0.28.3)
bash scripts/gen_uniffi_cs.sh

# 3. Build & run the C# frontend (net8.0, x64). Copies the cdylib next to the exe
#    as uniffi_voice_agent_core.dll. Emits voice-agent.exe.
dotnet build win/VoiceAgentCli/VoiceAgentCli.csproj -c Release
win/VoiceAgentCli/bin/Release/net8.0-windows/voice-agent.exe --config configs/gallium.yaml
```

- `win/VoiceAgentCli/Program.cs` — REPL with two modes toggled by **Shift+Tab**: `text` ⇄ `listen`. Commands: `/listen`, `/reset`, `/history`, `/help`, `/quit`.
- `win/VoiceAgentCli/SpeechInput.cs` — STT via `System.Speech`. `win/VoiceAgentCli/VoiceOutput.cs` — TTS via `System.Speech.Synthesis`.
- `win/VoiceAgentCli/DotEnv.cs` — loads a local `.env` at startup. `win/VoiceAgentCli/AppConfig.cs` — YAML loader (config resolution: `--config` → `VOICE_AGENT_CONFIG` → `~/.cache/voice-agent/config.yml` → `configs/gallium.yaml`).

## ACP client mode (`lib/src/acp_client.rs`)

`agent_new` spawns the backend and drives it as a **whole-turn** ACP client over
line-delimited JSON-RPC 2.0 on the child's stdio — the mirror of the
codex-app-server protocol.

| Method | Direction | Purpose |
|--------|-----------|---------|
| `initialize` | out | capability negotiation |
| `thread/start` | out | open a thread (cwd, model, developer instructions, approval policy, MCP config) |
| `turn/start` | out | run one turn, block until it completes |
| `item/tool/call` | **in** | backend invokes one of voice-agent's client tools |
| `item/{commandExecution,fileChange}/requestApproval` | **in** | backend asks voice-agent to permit a mutation |
| `item/completed` | **in** | carries the turn's final `agentMessage` text |

Key points:

- **The transport is bidirectional** (`rpc.rs`): inbound requests are dispatched on their own threads so a `turn/start` in flight can be answered by client-tool calls the backend originates.
- `config.mcp_servers` is forwarded to the backend via `thread/start`'s `config.mcp_servers`; the backend connects them.
- Known degradations vs. the old in-process agent: `step` returns text only (no keyword hints / token counts); `observe`/`step_with_allowed_tools` can't restrict the backend's own tool set (advisory only).

## Project Structure

```
voice-agent/
├── configs/                    # gallium.yaml (local), codex.yaml (cloud), system-prompt.md
├── skills/                     # project-local skills
├── crates/lib/src/             # voice_agent_core (cdylib): ACP client, client tools, orchestration
├── swift/Sources/              # VoiceAgentCli, AgentKit, Audio, TTS, ScreenCapture, Util, AgentBridge(FFI)
├── win/VoiceAgentCli/          # C# frontend
├── scripts/                    # gen_uniffi{,_cs}.sh, build-win-local.bat, build-ios.sh
└── docs/                       # REFACTOR.md, VOICE_PROCESSING_IO.md
```

## Troubleshooting

**"library 'voice_agent_core' not found"**: `cd crates && cargo build --release`

**"no such module 'voice_agent_coreFFI'"**: `bash scripts/gen_uniffi.sh`

**UniFFI checksum mismatch**: regenerate and the script copies for you: `bash scripts/gen_uniffi.sh`

**"spawn backend 'gallium': No such file"**: the backend isn't on PATH. Build/install `gallium` from `../rs-gallium`, or set `VOICE_AGENT_ACP_BACKEND` to another codex-app-server binary (e.g. `codex`).

**Model OOM / local inference issues**: these are the **backend's** concern now — tune the model/quant in the backend (`../rs-gallium`). voice-agent only forwards `MODEL_PATH`/`INFERENCE_ENGINE`.
