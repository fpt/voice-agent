# Voice Agent - Developer Documentation

## Overview

A fully functional local voice agent for macOS featuring natural voice interaction with local LLM processing. While designed with in-car use in mind, it works great in any environment.

### Current Status: **Phases 1-4 Complete** ✅

- ✅ **Phase 1**: Foundation (CLI, Config, Rust Integration)
- ✅ **Phase 2**: Real LLM Integration (UniFFI, gpt-oss-20B via llama.cpp)
- ✅ **Phase 3**: TTS (Voice output with AVSpeechSynthesizer)
- ✅ **Phase 4**: STT (Voice input with WhisperKit)
- ⏳ **Phase 5**: Echo Cancellation (planned)
- 📦 **Bonus**: Foundation Models-compatible protocol (on `foundation` branch)

### Key Features

- **Local-first**: All processing happens on-device (STT, LLM, TTS)
- **Voice I/O**: Full voice conversation loop implemented
- **Harmony Template**: Optimized for gpt-oss-20B model
- **Half-duplex**: TTS mutes input to prevent echo
- **WhisperKit**: On-device transcription with VAD
- **Configurable**: YAML-based configuration system
- **Future-proof**: Protocol-based architecture (foundation branch)

## Architecture

### Current System Flow

```
User speaks
    ↓
AVAudioEngine (microphone capture)
    ↓
WhisperKit (Speech-to-Text)
    ↓
Swift CLI (main loop)
    ↓
AgentBridge (UniFFI)
    ↓
Rust Agent Core
    ↓
LlmClient (HTTP)
    ↓
llama.cpp server
    ↓
gpt-oss-20B model
    ↓
Harmony template response
    ↓
HarmonyParser (extract final)
    ↓
AVSpeechSynthesizer (TTS)
    ↓
User hears response
```

### Technology Stack

#### Audio Pipeline (Swift)
- **Input**: AVAudioEngine with microphone capture
- **STT**: WhisperKit (CoreML-based, on-device)
  - Models: tiny, base, small, medium, large
  - Voice Activity Detection included
  - Automatic model download
- **TTS**: AVSpeechSynthesizer
  - Multiple voices available
  - Configurable rate, pitch, volume
  - Half-duplex control (mutes during speech)

#### Agent Core (Rust)
- **Agent**: Simple conversation loop with memory
- **LLM Client**: OpenAI-compatible HTTP client (ureq)
- **Concurrency**: Crossbeam + parking_lot (no tokio)
- **Memory**: Thread-safe conversation history
- **Harmony**: Template-aware response handling

#### LLM Backend
- **Server**: llama.cpp with HTTP API
- **Model**: gpt-oss-20B (Mixture of Experts)
- **Template**: Harmony (analysis + final channels)
- **Quantization**: MXFP4 for 16GB Macs
- **Context**: 8192 tokens
- **Max Tokens**: 4096 (configurable)

#### Integration
- **UniFFI**: Direct Rust↔Swift integration
- **C FFI**: Generated bindings in `vendor/uniffi-swift/`
- **System Library**: SPM systemLibrary target
- **Async/Await**: Full async support on Swift side

### Project Structure

```
voice-agent/
├── README.md                      # User-facing documentation
├── CLAUDE.md                      # This file (developer docs)
├── UNIFFI_SUCCESS.md              # Phase 2 completion notes
├── TTS_SUCCESS.md                 # Phase 3 completion notes
├── STT_SUCCESS.md                 # Phase 4 completion notes
├── configs/
│   └── default.yaml               # Runtime configuration
├── models/                        # GGUF models (gitignored)
├── scripts/
│   ├── start_llm.sh               # Start llama.cpp server
│   ├── test_integration.sh        # Integration test
│   ├── gen_uniffi.sh              # Generate UniFFI bindings
│   └── download_whisper_model.sh  # Model info (auto-downloads)
├── docs/
│   ├── CONFIGURATION.md           # Complete config reference
│   ├── MAX_TOKENS_FIX.md          # maxTokens troubleshooting
│   ├── LANGUAGE_CLIENT.md         # Foundation Models protocol (foundation branch)
│   └── LANGUAGE_CLIENT_SUCCESS.md # Protocol implementation notes
├── crates/                        # Rust workspace
│   ├── Cargo.toml                 # Workspace manifest
│   └── agent-core/                # Core agent library
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs             # UniFFI exports
│       │   ├── agent.rs           # Agent implementation
│       │   ├── agent.udl          # UniFFI interface
│       │   ├── llm.rs             # HTTP LLM client
│       │   ├── memory.rs          # Conversation memory
│       │   └── harmony.rs         # Harmony template
│       └── uniffi-bindgen-swift.rs # Binding generator
├── swift/                         # Swift Package Manager workspace
│   ├── Package.swift              # SPM manifest
│   └── Sources/
│       ├── VoiceAgentCLI/         # Main executable
│       │   └── main.swift         # Async entry point
│       ├── Util/                  # Utilities
│       │   ├── Config.swift       # YAML configuration
│       │   ├── Logging.swift      # Logger
│       │   └── HarmonyParser.swift # Parse Harmony output
│       ├── TTS/                   # Text-to-Speech
│       │   └── TextToSpeech.swift # AVSpeechSynthesizer wrapper
│       ├── STT/                   # Speech-to-Text
│       │   └── SpeechToText.swift # WhisperKit wrapper
│       ├── AgentBridgeFFI/        # System library
│       │   └── module.modulemap   # C module definition
│       ├── AgentBridge/           # Rust bridge
│       │   └── agent_core.swift   # Generated UniFFI bindings
│       └── LLM/                   # Foundation Models protocol (foundation branch)
│           ├── LanguageClient.swift      # Protocol definition
│           ├── OpenAICompatClient.swift  # Direct HTTP client
│           └── RustBridgeAdapter.swift   # Rust wrapper
└── vendor/                        # Generated code
    └── uniffi-swift/              # UniFFI outputs
        ├── agent_core.swift       # Swift bindings
        ├── agent_coreFFI.h        # C header
        └── agent_core.modulemap   # Module map
```

## Implementation Phases

### Phase 1: Foundation ✅

**Goal**: CLI with text I/O, config loading, Rust agent integration

**Completed:**
- Rust workspace with agent-core library
- Swift CLI with YAML configuration
- Simple conversation loop
- Logging system

**Key Files:**
- `crates/agent-core/src/lib.rs`
- `swift/Sources/VoiceAgentCLI/main.swift`
- `swift/Sources/Util/Config.swift`

### Phase 2: Real LLM Integration ✅

**Goal**: Connect to llama.cpp, use Harmony template, UniFFI bridge

**Completed:**
- UniFFI bindings generated successfully
- Rust agent calls llama.cpp HTTP API
- Harmony template support
- gpt-oss-20B model integration
- Conversation memory

**Key Files:**
- `crates/agent-core/src/agent.udl` (UniFFI interface)
- `crates/agent-core/src/llm.rs` (HTTP client)
- `crates/agent-core/src/harmony.rs` (Template)
- `vendor/uniffi-swift/` (Generated bindings)

**Documentation:** `UNIFFI_SUCCESS.md`

### Phase 3: TTS (Text-to-Speech) ✅

**Goal**: Voice output with AVSpeechSynthesizer

**Completed:**
- AVSpeechSynthesizer wrapper
- Voice selection system
- Configurable rate, pitch, volume
- Half-duplex control (mutes input during speech)
- Commands: `/voices`, `/stop`

**Key Files:**
- `swift/Sources/TTS/TextToSpeech.swift`
- `configs/default.yaml` (tts section)

**Documentation:** `TTS_SUCCESS.md`

### Phase 4: STT (Speech-to-Text) ✅

**Goal**: Voice input with WhisperKit

**Completed:**
- WhisperKit integration
- AVAudioEngine for microphone capture
- Voice Activity Detection (VAD)
- Audio buffer management
- Async/await architecture
- Command: `/listen`
- Automatic model download

**Key Files:**
- `swift/Sources/STT/SpeechToText.swift`
- `swift/Sources/VoiceAgentCLI/main.swift` (async refactor)
- `configs/default.yaml` (stt section)

**Requirements:**
- macOS 14.0+ (WhisperKit requirement)
- Microphone permissions

**Documentation:** `STT_SUCCESS.md`

### Phase 5: Echo Cancellation ⏳

**Goal**: Prevent assistant's voice from being picked up by microphone

**Planned Approaches:**

1. **Current: Half-duplex** (implemented)
   - Mute microphone during TTS playback
   - Simple and effective
   - Works well for turn-taking conversations

2. **Future: VoiceProcessingIO**
   - Use macOS `kAudioUnitSubType_VoiceProcessingIO`
   - Built-in AEC (Acoustic Echo Cancellation)
   - Feed TTS as "far-end" reference
   - Better for more natural interaction

3. **Advanced: WebRTC APM**
   - Link WebRTC Audio Processing library
   - Maximum control
   - Most complex implementation

### Bonus: Foundation Models Protocol 📦

**Goal**: Future-proof architecture aligned with Apple's API

**Branch:** `foundation`

**Completed:**
- `LanguageClient` protocol definition
- `OpenAICompatClient` (direct HTTP)
- `RustBridgeAdapter` (wraps existing bridge)
- Streaming support via AsyncThrowingStream
- Structured JSON output
- Full documentation

**Benefits:**
- Easy to switch backends (llama.cpp, OpenAI, Apple)
- Testable with mock implementations
- Clean separation of concerns
- Ready for Apple Foundation Models when 3B+ available

**Documentation:**
- `docs/LANGUAGE_CLIENT.md`
- `docs/LANGUAGE_CLIENT_SUCCESS.md`

## Configuration

### configs/default.yaml

```yaml
llm:
  baseURL: "http://127.0.0.1:8080/v1"   # llama.cpp server endpoint
  model: "gpt-oss-20b"                  # Model name
  apiKey: ""                            # Optional (empty for local)
  harmonyTemplate: true                 # Use Harmony format
  temperature: 0.7                      # Sampling temperature
  maxTokens: 4096                       # Max response tokens

agent:
  systemPromptPath: null                # Custom system prompt
  maxTurns: 50                          # Max conversation turns

tts:
  enabled: true                         # Enable voice output
  voice: null                           # Voice ID (null=default)
  rate: 0.5                             # Speech rate (0.0-1.0)
  pitchMultiplier: 1.0                  # Pitch (0.5-2.0)
  volume: 1.0                           # Volume (0.0-1.0)

stt:
  enabled: true                         # Enable voice input
  model: "base"                         # Whisper model size
  language: "en"                        # Language code
  silenceThreshold: -40.0               # Audio level in dB
  silenceDuration: 1.5                  # Silence timeout (seconds)
```

**See:** `docs/CONFIGURATION.md` for complete reference

## Development Workflow

### Setup

1. **Install Rust**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Install Xcode Command Line Tools**
   ```bash
   xcode-select --install
   ```

3. **Clone llama.cpp** (if not already installed)
   ```bash
   brew install llama.cpp
   # OR build from source
   git clone https://github.com/ggerganov/llama.cpp
   cd llama.cpp && make
   ```

4. **Download gpt-oss-20B model**
   ```bash
   huggingface-cli download ggml-org/gpt-oss-20b-GGUF \
     --include '*mxfp4.gguf' \
     --local-dir models/
   ```

### Build & Run

1. **Start llama.cpp server** (in one terminal)
   ```bash
   bash scripts/start_llm.sh
   ```

2. **Build Rust agent** (once, or after Rust changes)
   ```bash
   cd crates
   cargo build --release
   ```

3. **Generate UniFFI bindings** (after Rust API changes)
   ```bash
   bash scripts/gen_uniffi.sh
   ```

4. **Build and run Swift CLI**
   ```bash
   cd swift
   swift build
   swift run voice-agent --config ../configs/default.yaml
   ```

### Testing

**Integration test:**
```bash
bash scripts/test_integration.sh
```

**Manual text test:**
```bash
echo "What is 2+2?" | swift run voice-agent
```

**Voice test:**
```bash
swift run voice-agent --config ../configs/default.yaml
# Type: /listen
# Speak into microphone
# Press Enter to stop
```

## Commands

### CLI Commands

- `/help` - Show available commands
- `/quit` - Exit the program
- `/reset` - Clear conversation history
- `/history` - Show conversation
- `/voices` - List available TTS voices
- `/stop` - Stop current TTS playback
- `/listen` - Start voice input (STT mode)
- `/text` - Switch to text input mode

### Command Line Options

```bash
voice-agent [OPTIONS]

Options:
  --config PATH      Configuration file (default: configs/default.yaml)
  --verbose, -v      Enable verbose logging
  --help, -h         Show help message
```

## Key Technical Decisions

### Why Rust for Agent Core?
- **Performance**: Fast HTTP client with ureq
- **Safety**: Memory-safe conversation state
- **UniFFI**: Official Swift interop
- **Crossbeam**: Efficient lock-free concurrency
- **Future-proof**: Easy to add tool calling, MCP support

### Why llama.cpp over Ollama?
- **MoE Support**: Better gpt-oss performance
- **FlashAttention**: Optimized implementation
- **Control**: Direct GGUF loading
- **Lighter**: No extra daemon

### Why WhisperKit?
- **Native**: Pure Swift/CoreML
- **On-device**: No network required
- **Apple Silicon**: Metal-optimized
- **Streaming**: Built-in partial results
- **VAD**: Voice activity detection included

### Why AVSpeechSynthesizer?
- **Built-in**: No dependencies
- **Offline**: Works anywhere
- **App Store**: No licensing issues
- **Quality**: Good enough for MVP

### Why UniFFI?
- **Official**: Mozilla-supported
- **Type-safe**: Generated bindings
- **Performance**: Direct C FFI
- **Maintained**: Active development

### Why Crossbeam over Tokio?
- **Simpler**: No async runtime needed
- **Lighter**: Less overhead
- **Blocking**: Ureq HTTP client works well
- **Sufficient**: Agent loop is I/O bound

## Performance

### Current Metrics

- **STT Latency**: 1-3 seconds (base model)
- **LLM Latency**: 5-10 seconds (depends on complexity)
- **TTS Latency**: ~100-200ms to first audio
- **Memory**: ~2GB total (1GB model, 1GB other)
- **Context**: 8192 tokens
- **Max Response**: 4096 tokens

### Optimization Tips

1. **Faster STT**: Use tiny model (40MB)
2. **Faster LLM**: Lower maxTokens, increase temperature
3. **Faster TTS**: Increase speech rate
4. **Less Memory**: Use smaller Whisper model

## Troubleshooting

### Common Issues

**"library 'agent_core' not found"**
```bash
cd crates
cargo build --release
```

**"no such module 'agent_coreFFI'"**
```bash
bash scripts/gen_uniffi.sh
```

**"Failed to initialize WhisperKit"**
- Check internet connection (first download)
- Verify macOS 14.0+
- Try smaller model (tiny or base)

**"No audio data recorded"**
- Check microphone permissions
- Test microphone in System Settings
- Verify correct input device selected

**"Response ends without final answer"**
- Increase `maxTokens` in config (try 8192)
- See: `docs/MAX_TOKENS_FIX.md`

**Complete troubleshooting:** See `docs/CONFIGURATION.md`

## Documentation

### Core Docs
- `README.md` - User-facing overview
- `CLAUDE.md` - This file (developer guide)
- `docs/CONFIGURATION.md` - Complete config reference

### Phase Completion Notes
- `UNIFFI_SUCCESS.md` - Phase 2 (LLM integration)
- `TTS_SUCCESS.md` - Phase 3 (Voice output)
- `STT_SUCCESS.md` - Phase 4 (Voice input)

### Advanced Topics
- `docs/MAX_TOKENS_FIX.md` - Harmony template & tokens
- `docs/LANGUAGE_CLIENT.md` - Foundation Models protocol
- `docs/LANGUAGE_CLIENT_SUCCESS.md` - Protocol implementation

## Future Enhancements

### Short-term
- [ ] Echo cancellation with VoiceProcessingIO
- [ ] Push-to-talk mode (in addition to /listen)
- [ ] Conversation history persistence
- [ ] System prompt customization UI

### Medium-term
- [ ] Tool calling (file operations)
- [ ] MCP (Model Context Protocol) support
- [ ] Multiple conversation sessions
- [ ] Export/import conversations
- [ ] Kokoro TTS (higher quality)

### Long-term
- [ ] iOS + CarPlay version
- [ ] Wake word detection
- [ ] Multi-modal (camera input)
- [ ] Fine-tuned gpt-oss for conversations
- [ ] Cloud sync (optional)

## Contributing

This is currently a personal project. If you're interested in contributing:

1. Open an issue to discuss your idea
2. Fork the repository
3. Create a feature branch
4. Submit a pull request

Please follow the existing code style and add tests where appropriate.

## License

[To be determined]

## Resources

### Official Documentation
- [WhisperKit](https://github.com/argmaxinc/WhisperKit)
- [llama.cpp](https://github.com/ggerganov/llama.cpp)
- [UniFFI](https://mozilla.github.io/uniffi-rs/)
- [AVSpeechSynthesizer](https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer)
- [Apple Foundation Models](https://developer.apple.com/documentation/foundationmodels)

### Project Resources
- [gpt-oss Model Card](https://huggingface.co/ggml-org/gpt-oss-20b-GGUF)
- [Harmony Template](https://github.com/openai/gpt-oss)
- [CHAT.md](doc/CHAT.md) - Original planning notes

## Acknowledgments

- OpenAI for gpt-oss model
- Mozilla for UniFFI
- Argmax for WhisperKit
- llama.cpp contributors
- Everyone who shared echo cancellation techniques
