# Voice Agent 🎤🤖

A fully functional local voice assistant for macOS with complete voice interaction capabilities. Talk to it, and it talks back!

## Current Status: **Production Ready** ✅

**Phases 1-4 Complete:**
- ✅ Foundation (CLI, Config, Logging)
- ✅ LLM Integration (gpt-oss-20B via llama.cpp)
- ✅ Voice Output (TTS with AVSpeechSynthesizer)
- ✅ Voice Input (STT with WhisperKit)

**Try it now:**
```bash
swift run voice-agent --config ../configs/default.yaml
# Type: /listen
# Speak your question
# Press Enter
# Hear the response!
```

## Features

### 🔊 Voice Interaction
- **Speech-to-Text**: WhisperKit (on-device, CoreML-optimized)
- **Text-to-Speech**: AVSpeechSynthesizer (multiple voices)
- **Voice Activity Detection**: Automatic silence detection
- **Half-Duplex**: Mutes input during speech to prevent echo

### 🧠 LLM Processing
- **Model**: gpt-oss-20B (Mixture of Experts, 20B parameters)
- **Runtime**: llama.cpp server (FlashAttention, Metal acceleration)
- **Template**: Harmony (analysis + final response channels)
- **Local**: All processing happens on your Mac

### ⚙️ Configuration
- **YAML-based**: Easy configuration in `configs/default.yaml`
- **Customizable**: Voice, speed, model, tokens, etc.
- **Commands**: `/listen`, `/voices`, `/reset`, `/history`, and more

### 🏗️ Architecture
- **Rust Core**: Fast, memory-safe agent implementation
- **Swift CLI**: Native macOS interface
- **UniFFI Bridge**: Direct Rust↔Swift integration
- **Async/Await**: Modern Swift concurrency

## Quick Start

### Prerequisites

- macOS 14.0+ (for WhisperKit)
- 16GB RAM recommended
- Microphone access

### Setup

1. **Install dependencies**
   ```bash
   # Rust
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

   # Xcode Command Line Tools
   xcode-select --install

   # llama.cpp
   brew install llama.cpp
   ```

2. **Download model** (~5GB)
   ```bash
   huggingface-cli download ggml-org/gpt-oss-20b-GGUF \
     --include '*mxfp4.gguf' \
     --local-dir models/
   ```

3. **Start llama.cpp server** (in separate terminal)
   ```bash
   bash scripts/start_llm.sh
   ```

4. **Build and run**
   ```bash
   cd crates
   cargo build --release
   cd ../swift
   swift build
   swift run voice-agent --config ../configs/default.yaml
   ```

### First Conversation

```bash
$ swift run voice-agent --config ../configs/default.yaml

===========================================
  Voice Agent - Text Mode (Phase 1)
===========================================

Model: gpt-oss-20b
Endpoint: http://127.0.0.1:8080/v1

Type your messages below. Commands:
  /reset    - Clear conversation history
  /quit     - Exit the program
  /help     - Show this help
  /history  - Show conversation history
  /voices   - List available TTS voices
  /stop     - Stop current TTS playback
  /listen   - Start voice input (STT mode)
  /text     - Switch to text input mode

===========================================

You: /listen
🎤 Listening... (speak now, will auto-stop after silence)
Press Enter to stop recording manually...

[You speak: "What is the capital of France?"]
[Press Enter]

Transcribed: What is the capital of France?
Assistant: The capital of France is Paris.

[Response is also spoken aloud]
```

## Usage

### Using OpenAI (Cloud API)

Instead of running llama.cpp locally, you can use OpenAI's cloud API:

```bash
# 1. Build with OpenAI provider
make build-openai

# 2. Set your API key
export OPENAI_API_KEY=sk-...

# 3. Run with OpenAI
make run-openai
```

**Or use inline:**
```bash
OPENAI_API_KEY=sk-... make run-openai
```

See [docs/OPENAI_SETUP.md](docs/OPENAI_SETUP.md) for complete guide.

### Voice Mode

```bash
# Start voice mode
You: /listen

# Speak into your microphone
# Press Enter when done speaking

# The assistant will:
# 1. Transcribe your speech
# 2. Process with LLM
# 3. Show the response
# 4. Speak the response aloud
```

### Text Mode

```bash
# Type normally
You: What is 2+2?
Assistant: 4

# Or pipe input
echo "Hello!" | swift run voice-agent
```

### Commands

- `/listen` - Start voice input
- `/voices` - List available TTS voices
- `/stop` - Stop current speech
- `/reset` - Clear conversation history
- `/history` - Show conversation
- `/quit` - Exit

### Configuration

Edit `configs/default.yaml`:

```yaml
llm:
  maxTokens: 4096        # Increase for complex questions
  temperature: 0.7       # Higher = more creative

tts:
  enabled: true          # Enable voice output
  rate: 0.5              # Speech speed (0.0-1.0)
  voice: null            # null = default voice

stt:
  enabled: true          # Enable voice input
  model: "base"          # tiny, base, small, medium, large
  language: "en"         # Language code
```

See [docs/CONFIGURATION.md](docs/CONFIGURATION.md) for complete reference.

## Architecture

### System Flow

```
User speaks → Microphone → WhisperKit (STT) → Swift CLI →
UniFFI → Rust Agent → llama.cpp → gpt-oss-20B →
Harmony Parser → AVSpeechSynthesizer (TTS) → Speaker
```

### Technology Stack

- **Swift**: CLI, Audio I/O, WhisperKit, AVSpeechSynthesizer
- **Rust**: Agent core, LLM client, conversation memory
- **UniFFI**: Rust↔Swift bridge (Mozilla)
- **llama.cpp**: LLM inference server
- **gpt-oss-20B**: Language model (OpenAI)
- **WhisperKit**: Speech-to-text (Argmax)

### Performance

- **STT**: 1-3 seconds (base model)
- **LLM**: 5-10 seconds (varies by question)
- **TTS**: ~200ms to first audio
- **Memory**: ~2GB total
- **Local**: No cloud, no internet needed

## Documentation

### User Guides
- [Configuration Guide](docs/CONFIGURATION.md) - Complete config reference
- [Troubleshooting](docs/CONFIGURATION.md#troubleshooting-configuration-issues) - Common issues

### Developer Guides
- [CLAUDE.md](CLAUDE.md) - Developer documentation
- [UNIFFI_SUCCESS.md](UNIFFI_SUCCESS.md) - LLM integration notes
- [TTS_SUCCESS.md](TTS_SUCCESS.md) - Voice output implementation
- [STT_SUCCESS.md](STT_SUCCESS.md) - Voice input implementation

### Advanced Topics
- [MAX_TOKENS_FIX.md](docs/MAX_TOKENS_FIX.md) - Complex question handling
- [LANGUAGE_CLIENT.md](docs/LANGUAGE_CLIENT.md) - Foundation Models protocol (foundation branch)

## Examples

### Simple Question
```bash
You: /listen
[Speak: "What is 2+2?"]
Assistant: 4
```

### Complex Question
```bash
You: /listen
[Speak: "Explain the differences between various sorting algorithms"]
Assistant: [Detailed explanation with analysis]
# Note: Increase maxTokens to 8192 for very complex questions
```

### Change Voice
```bash
You: /voices
# Lists all available voices

# Edit configs/default.yaml:
tts:
  voice: "com.apple.voice.compact.en-US.Samantha"

# Restart and enjoy the new voice!
```

## Roadmap

### Completed ✅
- [x] Text-based CLI
- [x] LLM integration (UniFFI + Rust)
- [x] Voice output (TTS)
- [x] Voice input (STT with VAD)
- [x] Conversation history
- [x] Configurable system
- [x] Harmony template parsing

### In Progress 🚧
- [ ] Echo cancellation (VoiceProcessingIO)
- [ ] Push-to-talk mode

### Planned 📋
- [ ] Tool calling (file operations)
- [ ] MCP (Model Context Protocol)
- [ ] Wake word detection
- [ ] iOS + CarPlay version
- [ ] Conversation persistence

## Troubleshooting

**llama.cpp server not responding**
```bash
# Check if server is running
curl http://127.0.0.1:8080/health

# Restart server
bash scripts/start_llm.sh
```

**No microphone input**
- Grant microphone permission in System Settings → Privacy & Security
- Check correct input device selected
- Test microphone in other apps first

**Response cuts off mid-answer**
- Increase `maxTokens` in config (try 8192)
- See [MAX_TOKENS_FIX.md](docs/MAX_TOKENS_FIX.md)

**WhisperKit initialization failed**
- Check internet (first download)
- Verify macOS 14.0+
- Try smaller model (tiny)

See [complete troubleshooting guide](docs/CONFIGURATION.md#troubleshooting-configuration-issues).

## Contributing

Interested in contributing? Great!

1. Open an issue to discuss your idea
2. Fork the repository
3. Create a feature branch
4. Submit a pull request

Please follow existing code style and add tests where appropriate.

## License

[To be determined]

## Acknowledgments

- OpenAI for gpt-oss model
- Mozilla for UniFFI
- Argmax for WhisperKit
- llama.cpp contributors
- Everyone building amazing local AI tools!

## Contact

[To be added]

---

**Made with ❤️ for local-first AI**
