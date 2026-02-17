# Voice Agent - Developer Guide

## Overview

A macOS voice assistant with local and cloud LLM support, continuous voice I/O, tool calling, and Claude Code activity monitoring.

- **Platform**: macOS 26+ (requires Apple SpeechTranscriber)
- **Swift**: swift-tools-version 6.1, `.swiftLanguageMode(.v5)` on all targets
- **Rust**: workspace in `crates/` with two members: `lib` (library) and `app` (binary)

## Architecture

```
Mic -> AVAudioEngine -> SpeechAnalyzer/SpeechTranscriber (STT)
    -> Swift CLI (main.swift)
    -> UniFFI bridge
    -> Rust Agent (lib.rs)
    -> ReAct loop (react.rs) with LLM provider + tool registry
    -> Response
    -> AVSpeechSynthesizer (TTS) -> Speaker
```

### Rust Crates (`crates/`)

| File | Purpose |
|------|---------|
| `lib/src/lib.rs` | Agent struct, UniFFI exports, provider factory |
| `lib/src/llm.rs` | LlmProvider trait, OpenAiProvider (Responses API) |
| `lib/src/llm_local.rs` | LlamaLocalProvider (in-process llama-cpp-2 FFI) |
| `lib/src/react.rs` | Provider-agnostic ReAct loop |
| `lib/src/tool.rs` | ToolRegistry, ToolHandler trait, ToolAccess trait, built-in tools |
| `lib/src/skill.rs` | SkillRegistry, lookup_skill tool |
| `lib/src/memory.rs` | ConversationMemory (thread-safe) |
| `lib/src/state_capsule.rs` | State capsule for context injection |
| `lib/src/state_updater.rs` | Rule-based state extraction from responses |
| `lib/src/harmony.rs` | Harmony template parser (for gpt-oss models) |
| `lib/src/agent.udl` | UniFFI interface definition |
| `app/src/main.rs` | Standalone Rust CLI (for testing without Swift) |

### Swift Packages (`swift/Sources/`)

| Package | Purpose |
|---------|---------|
| `VoiceAgentCLI` | Main entry point, text/voice mode, watcher integration |
| `Audio` | AudioCapture (mic -> SpeechTranscriber), VoiceProcessingIO |
| `TTS` | AVSpeechSynthesizer wrapper |
| `Watcher` | SessionJSONLWatcher, SocketReceiver, EventPipeline |
| `Util` | Config, Logger, HarmonyParser, SkillLoader, ModelDownloader |
| `AgentBridge` | Generated UniFFI Swift bindings |
| `AgentBridgeFFI` | C module map for FFI |
| `LLM` | LanguageClient protocol (experimental) |

### Key Patterns

- `ChatMessage` has `#[serde(skip)]` fields for tool state; use helper methods (`ChatMessage::user()`, `ChatMessage::assistant()`, etc.) not struct literals
- ReAct loop in `react.rs` is provider-agnostic; each provider serializes to its own wire format in `chat_with_tools()`
- OpenAI provider uses Responses API (`/v1/responses`) with `function_call`/`function_call_output` input items
- Local LLM tool calling: `apply_chat_template_oaicompat()` -> grammar-constrained generation -> `parse_response_oaicompat()`
- `ToolAccess` trait abstracts `ToolRegistry` and `FilteredToolRegistry` for restricted tool access
- Half-duplex: `AudioCapture.mute()`/`unmute()` drops audio buffers during TTS playback

## Configuration

YAML configs in `configs/`. System prompt supports `{language}` template variable.

```yaml
llm:
  modelPath: "../models/Qwen3-8B-Q4_K_M.gguf"  # For local provider
  modelRepo: "Qwen/Qwen3-8B-GGUF"              # HuggingFace repo (auto-download)
  modelFile: "Qwen3-8B-Q4_K_M.gguf"            # File in repo
  baseURL: "https://api.openai.com/v1"          # For OpenAI provider
  model: "gpt-5-mini"
  apiKey: ""                                     # Or OPENAI_API_KEY env var
  harmonyTemplate: false
  temperature: 0.7
  maxTokens: 2048
  reasoningEffort: "medium"                      # For reasoning models

agent:
  systemPromptPath: "system-prompt.md"           # Relative to config dir
  maxTurns: 50
  language: "en"                                 # "en" or "ja"

tts:
  enabled: true
  voice: "com.apple.voice.enhanced.en-US.Zoe"
  rate: 0.5
  pitchMultiplier: 1.0
  volume: 1.0

stt:
  enabled: true
  locale: "en-US"                                # BCP47 locale
  censor: false

watcher:
  enabled: true
  debounceInterval: 3.0
```

Provider selection logic: if `modelPath` is set -> `LlamaLocalProvider`; else if `baseURL` is set -> `OpenAiProvider`.

## Skills

Skills are `SKILL.md` files with YAML frontmatter loaded from:
1. `skills/` directory (relative to config file's parent)
2. `~/.claude/plugins/` (recursive)

The `claude-activity-report` skill is used by the watcher via `chat_once(input, skillName:)`.

## Build & Run

```bash
# Rust
cd crates && cargo build --release
cd crates && cargo test

# UniFFI (after .udl changes)
bash scripts/gen_uniffi.sh
cp vendor/uniffi-swift/agent_core.swift swift/Sources/AgentBridge/

# Swift
cd swift && swift build

# Run
cd swift && swift run voice-agent --config ../configs/openai.yaml
cd swift && swift run voice-agent --config ../configs/qwen3.yaml

# Local model standalone (Rust only, no Swift)
MODEL_PATH=../models/Qwen3-8B-Q4_K_M.gguf cargo run -p app
```

## Claude Code Watcher

Monitors Claude Code via hooks (PostToolUse, Stop events) sent over a Unix domain socket.

- **Hook script**: `scripts/voice-agent-hook.sh` forwards stdin JSON to `/tmp/voice-agent-<uid>.sock`
- **Install**: `bash scripts/install-voice-agent-hook.sh` copies hook and updates `~/.claude/settings.json`
- **SocketReceiver** (`swift/Sources/Watcher/`): listens on the socket, parses ndjson
- **EventPipeline**: debounces events, summarizes via `EventSummarizer`, calls `agent.chatOnce()` with the `claude-activity-report` skill
- **SessionJSONLWatcher**: also watches Claude Code's session JSONL file for events

## Project Structure

```
voice-agent/
├── configs/                    # YAML configurations
│   ├── default.yaml            # Default (OpenAI, English)
│   ├── openai.yaml             # OpenAI with watcher
│   ├── openai-ja.yaml          # OpenAI, Japanese
│   ├── qwen3.yaml              # Local Qwen3-8B
│   └── system-prompt.md        # System prompt template ({language})
├── skills/                     # Project-local skills
│   └── claude-activity-report/SKILL.md
├── crates/                     # Rust workspace
│   ├── lib/src/                # Agent core library (agent_core)
│   └── app/src/                # Standalone Rust CLI
├── swift/                      # Swift package
│   └── Sources/
│       ├── VoiceAgentCLI/      # Main entry point
│       ├── Audio/              # SpeechTranscriber, AudioCapture
│       ├── TTS/                # AVSpeechSynthesizer
│       ├── Watcher/            # Claude Code monitoring
│       ├── Util/               # Config, Logger, SkillLoader
│       ├── AgentBridge/        # UniFFI Swift bindings
│       └── AgentBridgeFFI/     # C module map
├── scripts/
│   ├── gen_uniffi.sh           # Generate UniFFI bindings
│   ├── install-voice-agent-hook.sh  # Install Claude Code hook
│   ├── voice-agent-hook.sh          # Hook script (stdin -> socket)
│   └── ...
├── vendor/uniffi-swift/        # Generated UniFFI outputs
└── models/                     # GGUF models (gitignored)
```

## Troubleshooting

**"library 'agent_core' not found"**: `cd crates && cargo build --release`

**"no such module 'agent_coreFFI'"**: `bash scripts/gen_uniffi.sh`

**UniFFI checksum mismatch**: Regenerate bindings and copy: `bash scripts/gen_uniffi.sh && cp vendor/uniffi-swift/agent_core.swift swift/Sources/AgentBridge/`

**Local model OOM**: Use a smaller quantization or model. Qwen3-8B Q4_K_M (5GB) works on M3 16GB.
