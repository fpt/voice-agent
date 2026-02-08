# LanguageClient Protocol

## Overview

The `LanguageClient` protocol provides a clean, modern interface for interacting with language models. It's designed to be compatible with Apple's Foundation Models API, making it easy to switch between different backend implementations.

## Design Principles

1. **Protocol-based**: Easy to swap implementations (OpenAI, llama.cpp, Apple Foundation Models, etc.)
2. **Async/await**: Modern Swift concurrency
3. **Streaming support**: Incremental response delivery
4. **Type-safe**: Structured JSON output with Codable
5. **Future-proof**: Aligned with Apple's Foundation Models interface

## Protocol Definition

```swift
public protocol LanguageClient: Sendable {
    /// Check if the model is available
    func availability() async -> ModelAvailability

    /// Simple one-shot generation
    func generate(system: String?, user: String) async throws -> String

    /// Streaming generation
    func stream(system: String?, user: String) -> AsyncThrowingStream<StreamChunk, Error>

    /// Structured JSON output
    func structured<T: Decodable>(system: String?, user: String, as type: T.Type) async throws -> T
}
```

## Implementations

### 1. OpenAICompatClient

Direct HTTP client for OpenAI-compatible APIs (llama.cpp, OpenAI, etc.)

**Advantages:**
- No intermediary layers
- Full control over requests
- Supports streaming
- Works with any OpenAI-compatible endpoint

**Usage:**
```swift
import LLM

let config = OpenAICompatClient.Config(
    baseURL: URL(string: "http://127.0.0.1:8080/v1")!,
    model: "gpt-oss-20b",
    maxTokens: 4096,
    temperature: 0.7,
    useHarmonyTemplate: true
)

let client = OpenAICompatClient(config)

// Check availability
let status = await client.availability()
guard case .available = status else {
    print("Model not available")
    return
}

// Simple generation
let response = try await client.generate(
    system: "You are a helpful assistant.",
    user: "What is 2+2?"
)
print(response)

// Streaming
for try await chunk in client.stream(system: nil, user: "Tell me a story") {
    print(chunk.text, terminator: "")
    if chunk.isTerminal { print() }
}

// Structured JSON
struct Answer: Decodable {
    let result: Int
    let explanation: String
}

let answer: Answer = try await client.structured(
    system: nil,
    user: "What is 2+2? Return as JSON with 'result' and 'explanation' fields.",
    as: Answer.self
)
print("Result: \(answer.result)")
```

### 2. RustBridgeAdapter

Wraps the existing Rust UniFFI bridge to conform to LanguageClient protocol.

**Advantages:**
- Uses existing Rust agent with conversation history
- No changes to Rust code needed
- Harmony template parsing included
- Gradual migration path

**Usage:**
```swift
import LLM
import AgentBridge

// Create Rust agent (existing code)
let agentConfig = AgentConfig(
    baseUrl: "http://127.0.0.1:8080/v1",
    model: "gpt-oss-20b",
    apiKey: nil,
    useHarmonyTemplate: true,
    temperature: 0.7,
    maxTokens: 4096
)
let agent = try agentNew(config: agentConfig)

// Wrap in adapter
let clientConfig = OpenAICompatClient.Config(
    baseURL: URL(string: "http://127.0.0.1:8080/v1")!,
    model: "gpt-oss-20b",
    useHarmonyTemplate: true
)
let client = RustBridgeAdapter(agent: agent, config: clientConfig)

// Use the same LanguageClient interface
let response = try await client.generate(system: nil, user: "Hello!")
```

## Future: Apple Foundation Models

When Apple Foundation Models becomes more capable (larger models), you can easily switch:

```swift
// Hypothetical future Apple Foundation Models client
class AppleFoundationModelsClient: LanguageClient {
    func availability() async -> ModelAvailability {
        // Check Apple Intelligence availability
    }

    func generate(system: String?, user: String) async throws -> String {
        // Use Apple's API
    }

    // ... other methods
}

// No changes needed to calling code!
let client: LanguageClient = AppleFoundationModelsClient()
let response = try await client.generate(system: nil, user: "Hello!")
```

## Migration Path

### Current Architecture

```
Swift CLI → AgentBridge (UniFFI) → Rust Agent → llama.cpp
```

### With LanguageClient (Option 1: Direct)

```
Swift CLI → OpenAICompatClient → llama.cpp
```

### With LanguageClient (Option 2: Hybrid)

```
Swift CLI → RustBridgeAdapter → AgentBridge → Rust Agent → llama.cpp
```

### Future (Apple Foundation Models)

```
Swift CLI → AppleFoundationModelsClient → Apple Intelligence
```

## Streaming Example

```swift
func streamingChat(client: LanguageClient, prompt: String) async throws {
    print("Assistant: ", terminator: "")

    for try await chunk in client.stream(system: nil, user: prompt) {
        if chunk.isTerminal {
            print()  // New line at end
        } else {
            print(chunk.text, terminator: "")
            fflush(stdout)
        }
    }
}

// Works with any LanguageClient implementation
try await streamingChat(client: openAIClient, prompt: "Tell me a joke")
try await streamingChat(client: rustBridgeAdapter, prompt: "Explain quantum computing")
```

## Structured Output Example

```swift
struct WeatherResponse: Decodable {
    let temperature: Double
    let condition: String
    let humidity: Int
}

func getWeather(client: LanguageClient, location: String) async throws -> WeatherResponse {
    return try await client.structured(
        system: "You are a weather API. Return JSON with temperature (celsius), condition, and humidity (%).",
        user: "What's the weather in \(location)?",
        as: WeatherResponse.self
    )
}

let weather = try await getWeather(client: client, location: "Tokyo")
print("Temperature: \(weather.temperature)°C")
print("Condition: \(weather.condition)")
print("Humidity: \(weather.humidity)%")
```

## Configuration Comparison

### OpenAI Direct

```yaml
llm:
  type: "openai"  # Future: add type field to config
  baseURL: "http://127.0.0.1:8080/v1"
  model: "gpt-oss-20b"
  maxTokens: 4096
  temperature: 0.7
```

### Rust Bridge (Current)

```yaml
llm:
  type: "rust-bridge"  # Uses existing AgentBridge
  baseURL: "http://127.0.0.1:8080/v1"
  model: "gpt-oss-20b"
  harmonyTemplate: true
  maxTokens: 4096
```

### Apple Foundation Models (Future)

```yaml
llm:
  type: "apple"  # When 3B+ models available
  model: "apple-intelligence-large"
  # No baseURL needed - uses Apple's on-device runtime
```

## Benefits

### 1. Testability

Easy to create mock clients for testing:

```swift
class MockLanguageClient: LanguageClient {
    func availability() async -> ModelAvailability { .available }
    func generate(system: String?, user: String) async throws -> String {
        return "Mock response for: \(user)"
    }
    // ... etc
}

// Test with mock
let testClient = MockLanguageClient()
let response = try await generateSummary(client: testClient, text: "...")
```

### 2. Flexibility

Switch implementations without changing calling code:

```swift
func askQuestion(_ client: LanguageClient, _ question: String) async throws -> String {
    return try await client.generate(system: nil, user: question)
}

// Works with any implementation
try await askQuestion(openAIClient, "What is AI?")
try await askQuestion(rustBridge, "What is AI?")
try await askQuestion(appleClient, "What is AI?")  // Future
```

### 3. Performance Options

Choose the right backend for your needs:

- **OpenAICompatClient**: Best for streaming, lowest latency
- **RustBridgeAdapter**: Best for conversation history, ReAct loops (future)
- **AppleFoundationModels**: Best for privacy, no server needed (future)

## Implementation Status

- ✅ LanguageClient protocol defined
- ✅ OpenAICompatClient implemented
- ✅ RustBridgeAdapter implemented
- ✅ Streaming support
- ✅ Structured JSON output
- ⏳ CLI updated to use new protocol (pending)
- ⏳ Configuration type selection (pending)
- ⏳ Apple Foundation Models client (future)

## Next Steps

1. **Update CLI** to use LanguageClient protocol
2. **Add type field** to config for selecting backend
3. **Performance testing** between direct and bridge
4. **Streaming UI** for better user experience
5. **Apple Foundation Models** when available

## See Also

- [CONFIGURATION.md](CONFIGURATION.md) - Configuration guide
- [UNIFFI_SUCCESS.md](../UNIFFI_SUCCESS.md) - Current Rust bridge
- [Apple Foundation Models](https://developer.apple.com/documentation/foundationmodels) - Apple's API
