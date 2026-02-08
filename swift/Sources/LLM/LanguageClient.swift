import Foundation

/// Model availability status
public enum ModelAvailability: Equatable, Sendable {
    case available
    case unavailable(reason: String)
}

/// A chunk of streamed text
public struct StreamChunk: Sendable {
    public let text: String
    public let isTerminal: Bool

    public init(text: String, isTerminal: Bool) {
        self.text = text
        self.isTerminal = isTerminal
    }
}

/// Protocol for language model clients
/// Aligned with Apple Foundation Models interface for future compatibility
public protocol LanguageClient: Sendable {
    /// Check if the model is available (e.g., server running, model loaded)
    func availability() async -> ModelAvailability

    /// Simple one-shot generation (full output)
    func generate(system: String?, user: String) async throws -> String

    /// Streaming generation (receive partial outputs incrementally)
    func stream(system: String?, user: String) -> AsyncThrowingStream<StreamChunk, Error>

    /// Structured JSON output: Have LLM return JSON and parse it
    func structured<T: Decodable>(system: String?, user: String, as type: T.Type) async throws -> T
}

/// Chat message for OpenAI-compatible APIs
public struct ChatMessage: Codable, Sendable {
    public let role: String
    public let content: String

    public init(role: String, content: String) {
        self.role = role
        self.content = content
    }
}
