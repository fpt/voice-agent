import Foundation

/// OpenAI-compatible language model client
/// Works with llama.cpp server, OpenAI API, and other compatible endpoints
public final class OpenAICompatClient: LanguageClient {
    public struct Config: Sendable {
        public var baseURL: URL              // e.g., http://127.0.0.1:8080/v1
        public var apiKey: String?           // Often not needed for llama.cpp
        public var model: String             // e.g., "gpt-oss-20b"
        public var requestTimeout: TimeInterval = 120
        public var maxTokens: Int = 4096
        public var temperature: Double = 0.7
        public var useHarmonyTemplate: Bool = false

        public init(
            baseURL: URL,
            apiKey: String? = nil,
            model: String,
            requestTimeout: TimeInterval = 120,
            maxTokens: Int = 4096,
            temperature: Double = 0.7,
            useHarmonyTemplate: Bool = false
        ) {
            self.baseURL = baseURL
            self.apiKey = apiKey
            self.model = model
            self.requestTimeout = requestTimeout
            self.maxTokens = maxTokens
            self.temperature = temperature
            self.useHarmonyTemplate = useHarmonyTemplate
        }
    }

    private let cfg: Config
    private let session: URLSession

    public init(_ cfg: Config) {
        self.cfg = cfg
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = cfg.requestTimeout
        config.timeoutIntervalForResource = cfg.requestTimeout
        self.session = URLSession(configuration: config)
    }

    // MARK: - Availability

    public func availability() async -> ModelAvailability {
        // Check if OpenAI-compatible /models endpoint responds
        do {
            var req = URLRequest(url: cfg.baseURL.appendingPathComponent("models"))
            if let key = cfg.apiKey {
                req.addValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
            }
            let (_, resp) = try await session.data(for: req)
            guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                return .unavailable(reason: "models endpoint returned status \((resp as? HTTPURLResponse)?.statusCode ?? -1)")
            }
            return .available
        } catch {
            return .unavailable(reason: error.localizedDescription)
        }
    }

    // MARK: - Generate (non-streaming)

    public func generate(system: String?, user: String) async throws -> String {
        let url = cfg.baseURL.appendingPathComponent("chat/completions")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.addValue("application/json", forHTTPHeaderField: "Content-Type")
        if let key = cfg.apiKey {
            req.addValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        }

        var messages: [ChatMessage] = []
        if let system, !system.isEmpty {
            messages.append(.init(role: "system", content: system))
        }
        messages.append(.init(role: "user", content: user))

        let payload: [String: Any] = [
            "model": cfg.model,
            "messages": messages.map { ["role": $0.role, "content": $0.content] },
            "max_tokens": cfg.maxTokens,
            "temperature": cfg.temperature,
            "stream": false
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: payload, options: [])

        let (data, resp) = try await session.data(for: req)
        try Self.ensureOK(resp, data: data)
        return try Self.parseText(from: data)
    }

    // MARK: - Stream

    public func stream(system: String?, user: String) -> AsyncThrowingStream<StreamChunk, Error> {
        AsyncThrowingStream { continuation in
            Task.detached { [cfg, session] in
                do {
                    let url = cfg.baseURL.appendingPathComponent("chat/completions")
                    var req = URLRequest(url: url)
                    req.httpMethod = "POST"
                    req.addValue("application/json", forHTTPHeaderField: "Content-Type")
                    if let key = cfg.apiKey {
                        req.addValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
                    }

                    var messages: [ChatMessage] = []
                    if let system, !system.isEmpty {
                        messages.append(.init(role: "system", content: system))
                    }
                    messages.append(.init(role: "user", content: user))

                    let payload: [String: Any] = [
                        "model": cfg.model,
                        "messages": messages.map { ["role": $0.role, "content": $0.content] },
                        "max_tokens": cfg.maxTokens,
                        "temperature": cfg.temperature,
                        "stream": true
                    ]
                    req.httpBody = try JSONSerialization.data(withJSONObject: payload, options: [])

                    let (bytes, resp) = try await session.bytes(for: req)
                    guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                        throw StreamError.httpStatus((resp as? HTTPURLResponse)?.statusCode ?? -1)
                    }

                    for try await line in bytes.lines {
                        // OpenAI-compatible streaming: "data: {json}\n" / "data: [DONE]"
                        guard line.hasPrefix("data:") else { continue }
                        let dataStr = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        if dataStr == "[DONE]" {
                            continuation.yield(.init(text: "", isTerminal: true))
                            continuation.finish()
                            break
                        }
                        if let chunk = Self.deltaText(fromSSEJSONLine: String(dataStr)) {
                            continuation.yield(.init(text: chunk, isTerminal: false))
                        }
                    }
                } catch {
                    continuation.finish(throwing: error)
                }
            }
        }
    }

    // MARK: - Structured JSON

    public func structured<T: Decodable>(system: String?, user: String, as type: T.Type) async throws -> T {
        // Request JSON output explicitly in the prompt
        let jsonHint = """
        Output must be valid JSON only. No extra text or explanations.
        """
        let combinedSystem = [system, jsonHint].compactMap { $0 }.joined(separator: "\n\n")

        let text = try await generate(system: combinedSystem, user: user)
        // Extract JSON part (handle extra text gracefully)
        let trimmed = Self.extractFirstJSONObject(from: text) ?? text
        let data = Data(trimmed.utf8)
        return try JSONDecoder().decode(T.self, from: data)
    }

    // MARK: - Helpers

    private static func ensureOK(_ resp: URLResponse, data: Data) throws {
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw SimpleError("HTTP \((resp as? HTTPURLResponse)?.statusCode ?? -1): \(body)")
        }
    }

    private static func parseText(from data: Data) throws -> String {
        struct ChoiceMsg: Decodable { let content: String }
        struct Choice: Decodable { let message: ChoiceMsg }
        struct Root: Decodable { let choices: [Choice] }
        let root = try JSONDecoder().decode(Root.self, from: data)
        return root.choices.first?.message.content ?? ""
    }

    private static func deltaText(fromSSEJSONLine line: String) -> String? {
        struct DeltaMsg: Decodable { let content: String? }
        struct Delta: Decodable { let delta: DeltaMsg? }
        struct Root: Decodable { let choices: [Delta] }
        guard let data = line.data(using: .utf8) else { return nil }
        if let root = try? JSONDecoder().decode(Root.self, from: data) {
            return root.choices.first?.delta?.content
        }
        return nil
    }

    private static func extractFirstJSONObject(from text: String) -> String? {
        // Simple extraction: find first { ... } with nesting support
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

    enum StreamError: Error {
        case httpStatus(Int)
    }

    struct SimpleError: Error, LocalizedError {
        let message: String
        init(_ message: String) { self.message = message }
        var errorDescription: String? { message }
    }
}
