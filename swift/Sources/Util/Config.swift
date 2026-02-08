import Foundation
import Yams

/// Configuration structure matching configs/default.yaml
public struct Config: Codable {
    public let llm: LLMConfig
    public let agent: AgentConfig
    public let tts: TTSConfig?
    public let stt: STTConfig?
    public let watcher: WatcherConfig?

    public struct LLMConfig: Codable {
        public let baseURL: String
        public let model: String
        public let apiKey: String?
        public let harmonyTemplate: Bool
        public let temperature: Float?
        public let maxTokens: Int
        public let modelPath: String?
        public let modelRepo: String?
        public let modelFile: String?
        public let reasoningEffort: String?

        enum CodingKeys: String, CodingKey {
            case baseURL = "baseURL"
            case model
            case apiKey
            case harmonyTemplate
            case temperature
            case maxTokens
            case modelPath
            case modelRepo
            case modelFile
            case reasoningEffort
        }
    }

    public struct AgentConfig: Codable {
        public let systemPromptPath: String?
        public let maxTurns: Int
        public let language: String?

        enum CodingKeys: String, CodingKey {
            case systemPromptPath
            case maxTurns
            case language
        }
    }

    public struct TTSConfig: Codable {
        public let enabled: Bool
        public let voice: String?
        public let rate: Float
        public let pitchMultiplier: Float
        public let volume: Float

        public init(
            enabled: Bool,
            voice: String?,
            rate: Float,
            pitchMultiplier: Float,
            volume: Float
        ) {
            self.enabled = enabled
            self.voice = voice
            self.rate = rate
            self.pitchMultiplier = pitchMultiplier
            self.volume = volume
        }

        enum CodingKeys: String, CodingKey {
            case enabled
            case voice
            case rate
            case pitchMultiplier
            case volume
        }
    }

    public struct STTConfig: Codable {
        public let enabled: Bool
        public let locale: String?              // BCP47 locale (default: current system locale)
        public let censor: Bool?                // Enable etiquette replacements

        public init(
            enabled: Bool,
            locale: String? = nil,
            censor: Bool? = nil
        ) {
            self.enabled = enabled
            self.locale = locale
            self.censor = censor
        }
    }

    public struct WatcherConfig: Codable {
        public let enabled: Bool
        public let debounceInterval: Double?    // seconds, default 3.0
        public let socketPath: String?          // Unix socket path (null=auto)
        public let sessionPath: String?         // JSONL path (null=auto-detect)
    }

    /// Load configuration from YAML file
    public static func load(from path: String) throws -> Config {
        let url = URL(fileURLWithPath: path)
        let data = try Data(contentsOf: url)
        let decoder = YAMLDecoder()
        return try decoder.decode(Config.self, from: data)
    }

    /// Default configuration for development
    public static func `default`() -> Config {
        Config(
            llm: LLMConfig(
                baseURL: "http://127.0.0.1:8080/v1",
                model: "gpt-oss-20b",
                apiKey: nil,
                harmonyTemplate: true,
                temperature: 0.7,
                maxTokens: 4096,
                modelPath: nil,
                modelRepo: nil,
                modelFile: nil,
                reasoningEffort: nil
            ),
            agent: AgentConfig(
                systemPromptPath: nil,
                maxTurns: 50,
                language: "en"
            ),
            tts: TTSConfig(
                enabled: false,
                voice: nil,
                rate: 0.5,
                pitchMultiplier: 1.0,
                volume: 1.0
            ),
            stt: STTConfig(
                enabled: false
            ),
            watcher: nil
        )
    }
}
