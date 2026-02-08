import Foundation
@preconcurrency import AgentBridge

/// Adapter that wraps the existing Rust UniFFI bridge to conform to LanguageClient protocol
/// This allows gradual migration while maintaining compatibility with the Rust agent
public final class RustBridgeAdapter: LanguageClient {
    private let agent: Agent
    private let config: OpenAICompatClient.Config

    public init(agent: Agent, config: OpenAICompatClient.Config) {
        self.agent = agent
        self.config = config
    }

    // MARK: - Availability

    public func availability() async -> ModelAvailability {
        // The Rust agent is created synchronously, so if we have it, it's available
        // We could add a health check to the Rust side in the future
        return .available
    }

    // MARK: - Generate

    public func generate(system: String?, user: String) async throws -> String {
        // Use the existing Rust agent's step method
        // Note: The Rust agent maintains conversation history internally
        let response = try agent.step(userInput: user)

        // Parse Harmony template if enabled
        if config.useHarmonyTemplate {
            return HarmonyParser.extractFinalResponse(response.content)
        }

        return response.content
    }

    // MARK: - Stream

    public func stream(system: String?, user: String) -> AsyncThrowingStream<StreamChunk, Error> {
        // Current Rust implementation doesn't support streaming
        // Fall back to non-streaming and yield all at once
        AsyncThrowingStream { continuation in
            Task {
                do {
                    let text = try await self.generate(system: system, user: user)
                    continuation.yield(.init(text: text, isTerminal: false))
                    continuation.yield(.init(text: "", isTerminal: true))
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
        }
    }

    // MARK: - Structured JSON

    public func structured<T: Decodable>(system: String?, user: String, as type: T.Type) async throws -> T {
        // Add JSON instruction to the user prompt
        let jsonPrompt = """
        \(user)

        Respond with valid JSON only. No explanations.
        """

        let text = try await generate(system: system, user: jsonPrompt)

        // Try to extract JSON if there's extra text
        let jsonText = Self.extractFirstJSONObject(from: text) ?? text
        let data = Data(jsonText.utf8)
        return try JSONDecoder().decode(T.self, from: data)
    }

    // MARK: - Helpers

    private static func extractFirstJSONObject(from text: String) -> String? {
        guard let start = text.firstIndex(of: "{") else { return nil }
        var depth = 0
        for i in text.indices[start..<text.endIndex] {
            let ch = text[i]
            if ch == "{" { depth += 1 }
            if ch == "}" {
                depth -= 1
                if depth == 0 {
                    return String(text[start...i])
                }
            }
        }
        return nil
    }
}

// Import HarmonyParser from Util
import Util

// Re-export for convenience
public typealias HarmonyParser = Util.HarmonyParser
