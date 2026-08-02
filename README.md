# voice-agent

A voice-and-text assistant that runs on **macOS and Windows**. voice-agent does
**no LLM inference of its own** — it is an **ACP client**: it spawns a backend
agent and drives it a turn at a time over JSON-RPC, serving its resident tools
(screen capture, the situation feed) back to the backend.

The frontend is native per platform; the agent backend is a separate binary
(`gallium` by default) found on PATH.

## Platforms

| | Frontend | Speech |
|---|---|---|
| **macOS 26+** | Swift — `voice-agent` | Apple SpeechTranscriber (STT) + AVSpeechSynthesizer (TTS) |
| **Windows** | C# / .NET 8 — `voice-agent.exe` | `System.Speech` recognizer + synthesizer |

The backend is swappable: `gallium` and `codex` both speak the same
codex-app-server JSON-RPC subset. Select it with `VOICE_AGENT_ACP_BACKEND` (default
`gallium`).

## Features

- **Backend-agnostic ACP client**: spawns and drives whatever app-server it's pointed at; no local inference or agent loop of its own
- **Screen awareness**: window capture / OCR client tools + an ambient situation feed
- **MCP**: `mcpServers` are forwarded to the backend, which connects them ([details](#mcp))
- **Voice I/O**: continuous conversation, half-duplex (mic muted during playback to prevent echo)
- **Multi-language**: English and Japanese, with configurable system-prompt templates

## Requirements

**Common**
- Rust toolchain
- A backend on PATH — build/install `gallium` from [`../rs-gallium`](https://github.com/fpt/rs-gallium), or point `VOICE_AGENT_ACP_BACKEND` at `codex`

**macOS**
- macOS 26+ (Apple SpeechTranscriber)
- Xcode Command Line Tools

**Windows**
- .NET 8 SDK (the Rust cdylib has no C++ deps — any toolchain works)

## Quick Start

### macOS

```bash
# Build the Rust core + Swift app and install `voice-agent` to ~/bin
make install

# Run against the local gallium backend (auto-downloads the model on first run)
voice-agent --config configs/gallium.yaml

# ...or a cloud backend
export OPENAI_API_KEY=sk-...
VOICE_AGENT_ACP_BACKEND=codex voice-agent --config configs/codex.yaml
```

`make install` installs a single binary, **`voice-agent`** (the voice app). The agent
backend (`gallium`) is installed separately from its own repo and found on PATH.

### Windows

```bash
# Build the Rust cdylib (voice_agent_core.dll), then the C# frontend
make build-win

# Generate the C# bindings once (see CLAUDE.md for the one-time install)
bash scripts/gen_uniffi_cs.sh

# Run
win/VoiceAgentCli/bin/Release/net8.0-windows/voice-agent.exe --config configs/gallium.yaml
```

The Windows REPL has two modes, toggled with **Shift+Tab**: `text` (type → printed
reply) and `listen` (speak → reply printed *and* spoken).

## Configuration

YAML configs live in `configs/`; the same schema is read by every frontend. Two
are shipped, one per backend flavor:

| config | backend | notes |
|--------|---------|-------|
| `gallium.yaml` | `gallium` (default) | local model via the standalone pure-Rust agent |
| `codex.yaml` | `codex` (cloud) | set `VOICE_AGENT_ACP_BACKEND=codex` + `OPENAI_API_KEY` |

```yaml
llm:                                  # forwarded to the backend as environment
  modelPath: "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf"  # local (MODEL_PATH)
  baseURL: "https://api.openai.com/v1"                            # cloud (LLM_BASE_URL)
  model: "gpt-5.6-luna"
  inferenceEngine: "gallium"          # backend's local engine: llamacpp | gallium
  temperature: 0.7
  maxTokens: 2048

agent:
  systemPromptPath: "system-prompt.md"  # supports the {language} variable
  maxTurns: 50
  language: "en"                        # "en" or "ja"

mcpServers:                             # forwarded to the backend, which connects them
  - command: "godevmcp"
    args: ["serve"]
  - url: "http://127.0.0.1:27182/mcp"

tts: { enabled: true, voice: "com.apple.voice.enhanced.en-US.Zoe" }
stt: { enabled: true, locale: "en-US" }
```

The `llm:` block is **forwarded to the backend** — voice-agent does not interpret it.
Backend selection is via `VOICE_AGENT_ACP_BACKEND` (env), not the config.

## MCP

voice-agent forwards the config's `mcpServers` to the backend over `thread/start`; the
**backend** connects them (stdio via `command`/`args`, or Streamable HTTP via
`url`) and exposes their tools to the model. A server that fails to connect is
logged and skipped.

```yaml
mcpServers:
  - command: "npx"                          # stdio
    args: ["-y", "@modelcontextprotocol/server-everything"]
  - url: "http://127.0.0.1:27182/mcp"       # Streamable HTTP
```

## ACP client

`voice-agent` spawns the backend (`gallium app-server` by default) and drives it as a
**whole-turn** ACP client over line-delimited JSON-RPC on stdio — it sends
`initialize`/`thread/start`/`turn/start` and handles the backend's inbound
`item/tool/call` + approval requests. The backend runs its own ReAct loop, tools,
and MCP connections inside each turn; voice-agent serves the screen capture and
situation reader back as `dynamicTools`.

Override the backend program with `VOICE_AGENT_ACP_BACKEND` (default `gallium`; may be
`"prog arg1 arg2"`). See [CLAUDE.md](CLAUDE.md) and [docs/REFACTOR.md](docs/REFACTOR.md).

## Skills

Skills load from `skills/` (project) and `~/.claude/plugins/`. Each is a `SKILL.md`
with YAML frontmatter; its catalog is injected into the backend thread's developer
instructions.

```markdown
---
name: my-skill
description: "What this skill does"
---
Prompt body injected as system context...
```

## Commands

**macOS (`voice-agent`)**

| Command | Description |
|---------|-------------|
| `/listen` | Listen once, then reply |
| `/goal <condition>` | Work toward a condition across turns; `/goal` for status, `/goal clear` to stop |
| `/loop [interval] <prompt>` | Ambient mode: run a prompt periodically in the background |
| `/reset` | Clear conversation history |
| `/history` | Show conversation |
| `/voices` | List available TTS voices |
| `/stop` | Stop current TTS playback |
| `/help` | Show help |
| `/quit` | Exit |

**Windows (`voice-agent.exe`)**

| Command | Description |
|---------|-------------|
| **Shift+Tab** | Toggle `text` ⇄ `listen` mode |
| `/listen` | Listen once, then reply |
| `/reset` | Clear conversation history |
| `/history` | Show conversation |
| `/help` | Show help |
| `/quit` | Exit |

## Architecture

```
macOS:    Mic -> SpeechTranscriber -> Swift CLI ─┐
Windows:  Mic -> System.Speech     -> C# CLI   ──┼─> UniFFI -> Rust ACP client
                                                 │              │  spawns + drives
                                                 │              v
                                                 │       gallium app-server
                                                 │       (ReAct + LLM + tools + MCP)
                                                 └───<── item/tool/call (capture, situation)
```

- **Rust** (`crates/lib`, `voice_agent_core`): ACP client, client tools, and local orchestration (goals, situation, backchannel). No inference.
- **Swift** (`swift/`): macOS voice app, audio pipeline, TTS.
- **C#** (`win/`): Windows frontend.
- **UniFFI**: generates the Swift and C# bindings to the Rust core.
- The agent backend (ReAct loop, LLM providers, tools, MCP) lives in [`../rs-gallium`](https://github.com/fpt/rs-gallium).

## Development

See [CLAUDE.md](CLAUDE.md) for the full developer guide.

```bash
cd crates && cargo build --release
cd crates && cargo test

# Regenerate UniFFI bindings after changing crates/lib/src/agent.udl
bash scripts/gen_uniffi.sh        # Swift (copies into the tracked tree)
bash scripts/gen_uniffi_cs.sh     # C#

cd swift && swift build
```

## License

MIT
