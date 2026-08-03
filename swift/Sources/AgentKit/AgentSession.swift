import Foundation
import AgentBridge
import Util
import TTS

/// Shared agent lifecycle — usable from CLI, iOS, or any other frontend.
public class AgentSession: @unchecked Sendable {

    // MARK: - Public properties

    /// The agent behind this session. A protocol, not the concrete UniFFI type,
    /// so a second backend (Foundation Models) can be substituted — see
    /// docs/FOUNDATION_MODELS.md.
    public let backend: AgentBackend
    public let tts: TextToSpeech
    public let config: Config
    public let language: String
    public let configPath: String

    // MARK: - Private state

    private let logger = Logger("AgentSession")

    // MARK: - Init

    /// Initialize agent, TTS, and load skills. `approver` gates the backend's
    /// mutation requests (file writes / shell commands); pass `nil` to let the
    /// backend run autonomously with no approval gate.
    public init(config: Config, configPath: String, approver: MutationApprover? = nil) async throws {
        self.config = config
        self.configPath = configPath
        self.language = config.agent.language ?? "en"

        // Resolve API key
        let apiKey: String? = {
            if let envKey = ProcessInfo.processInfo.environment["OPENAI_API_KEY"], !envKey.isEmpty {
                return envKey
            } else if let configKey = config.llm.apiKey, !configKey.isEmpty {
                return configKey
            }
            return nil
        }()

        // Resolve model path. `hf:ORG/REPO[@REV]/file.gguf` specs and absolute
        // paths pass through untouched (the Rust model downloader resolves `hf:`,
        // downloading into the HF cache); only bare relative paths are resolved
        // against the config dir.
        var modelPath: String? = nil
        if let cfgModelPath = config.llm.modelPath {
            if cfgModelPath.hasPrefix("hf:") || cfgModelPath.hasPrefix("/") {
                modelPath = cfgModelPath
            } else {
                let configDir = URL(fileURLWithPath: configPath).deletingLastPathComponent()
                modelPath = configDir.appendingPathComponent(cfgModelPath).path
            }
        }

        let mcpServers = (config.mcpServers ?? []).map {
            McpServerConfig(command: $0.command ?? "", args: $0.args ?? [], url: $0.url)
        }
        let agentConfig = AgentConfig(
            modelPath: modelPath,
            baseUrl: config.llm.baseURL,
            model: config.llm.model,
            apiKey: apiKey,
            temperature: config.llm.temperature,
            maxTokens: UInt32(config.llm.maxTokens),
            language: language,
            workingDir: FileManager.default.currentDirectoryPath,
            reasoningEffort: config.llm.reasoningEffort,
            inferenceEngine: config.llm.inferenceEngine,
            // "foundation-models" names an in-process backend, not a program on
            // PATH. Passing it through would make the Rust side try to spawn a
            // binary by that name if we end up falling back, so it is cleared
            // here and Rust applies its own default (gallium).
            backend: Self.wantsFoundationModels(config.backend) ? nil : config.backend,
            mcpServers: mcpServers
        )

        // `backend: "foundation-models"` runs Apple's on-device model in-process
        // instead of spawning an app-server. It is device-gated (Apple silicon +
        // Apple Intelligence enabled), so a request for it that the machine
        // cannot honour falls back rather than failing — see
        // docs/FOUNDATION_MODELS.md.
        var chosen: AgentBackend? = nil
        if Self.wantsFoundationModels(config.backend) {
            #if canImport(FoundationModels)
            if #available(macOS 26.0, *) {
                chosen = await MainActor.run { FoundationModelsBackend.make(screenTools: ScreenTools()) }
            } else {
                logger.warning("foundation-models needs macOS 26+; falling back to the app-server backend")
            }
            #else
            logger.warning("this build has no FoundationModels; falling back to the app-server backend")
            #endif
            if chosen != nil { logger.info("Backend: foundation-models (on-device, in-process)") }
        }

        if let fm = chosen {
            self.backend = fm
        } else {
            let agent = try agentNew(config: agentConfig, approver: approver)
            self.backend = AppServerBackend(agent: agent)
        }
        logger.info("Agent initialized")

        // TTS
        let ttsConfig = config.tts ?? Config.TTSConfig(
            enabled: false, voice: nil, rate: 0.5, pitchMultiplier: 1.0, volume: 1.0
        )
        let ttsVoice: String?
        if let v = ttsConfig.voice {
            ttsVoice = v
        } else {
            switch language {
            case "ja": ttsVoice = "com.apple.voice.enhanced.ja-JP.Kyoko"
            default: ttsVoice = "com.apple.voice.enhanced.en-US.Samantha"
            }
        }
        self.tts = TextToSpeech(config: TextToSpeech.Config(
            enabled: ttsConfig.enabled,
            voice: ttsVoice,
            rate: ttsConfig.rate,
            pitchMultiplier: ttsConfig.pitchMultiplier,
            volume: ttsConfig.volume
        ))

        // --- Post-init setup ---

        // Load system prompt with {language} template
        if let systemPromptPath = config.agent.systemPromptPath {
            var resolvedPath = systemPromptPath
            if !systemPromptPath.hasPrefix("/") {
                let configDir = URL(fileURLWithPath: configPath).deletingLastPathComponent()
                resolvedPath = configDir.appendingPathComponent(systemPromptPath).path
            }
            do {
                var systemPrompt = try String(contentsOfFile: resolvedPath, encoding: .utf8)
                let languagePrompt: String = {
                    switch language {
                    case "ja": return "日本語で回答してください。"
                    case "en": return ""
                    default: return "Respond in \(language)."
                    }
                }()
                systemPrompt = systemPrompt.replacingOccurrences(of: "{language}", with: languagePrompt)
                backend.setSystemPrompt(systemPrompt)
                logger.info("Loaded system prompt from \(resolvedPath)")
            } catch {
                logger.warning("Failed to load system prompt: \(error)")
            }
        }

        // Load skills from configured paths (relative to config dir)
        let configDir = URL(fileURLWithPath: configPath).deletingLastPathComponent().path
        let skillPaths = config.agent.skillPaths ?? ["skills"]
        let discoveredSkills = SkillLoader.loadAll(paths: skillPaths, baseDir: configDir)
        for skill in discoveredSkills {
            backend.addSkill(name: skill.name, description: skill.description, prompt: skill.prompt)
        }
        logger.info("Skills registered (\(discoveredSkills.count) from \(skillPaths))")
    }

    // MARK: - Lifecycle

    /// Whether the config asks for the in-process Foundation Models backend.
    /// Everything else names a program to spawn and is resolved in Rust.
    static func wantsFoundationModels(_ backend: String?) -> Bool {
        guard let b = backend?.trimmingCharacters(in: .whitespaces).lowercased(), !b.isEmpty else {
            return false
        }
        return b == "foundation-models" || b == "foundationmodels" || b == "apple"
    }

    /// Start background event sources. Currently a no-op — the Claude Code
    /// watcher was removed; ambient context is fed via `pushSituationMessage`
    /// from the frontend's window-list poller.
    public func start() {}

    /// Stop background resources. Currently a no-op (see `start()`).
    public func stop() {}

    // MARK: - Agent calls

    /// Run one conversation turn.
    public func step(_ text: String) async throws -> AgentResponse {
        try await backend.step(text)
    }

    /// Reset conversation history.
    public func reset() {
        backend.reset()
    }

    /// The conversation so far, formatted for display.
    public func conversationHistory() -> String {
        backend.conversationHistory()
    }

    /// Process a slash command. Returns true if handled.
    public func handleCommand(_ command: String) -> Bool {
        switch command {
        case "/reset":
            backend.reset()
            return true
        case "/voices":
            TextToSpeech.printAvailableVoices()
            return true
        case "/stop":
            tts.stop()
            return true
        default:
            return false
        }
    }

}
