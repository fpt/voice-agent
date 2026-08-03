import AgentBridge
import AgentCore
import Foundation
import FoundationModelsKit
import ScreenCapture
import Util
import TTS

/// A session that could not be started, phrased for a person.
public struct AgentSessionError: LocalizedError, CustomStringConvertible {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var errorDescription: String? { message }
    public var description: String { message }
}

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
            // Always the configured value: the in-process backend is handled
            // below and never reaches this path, because an unavailable one is
            // now fatal rather than a fallback to Rust's default.
            backend: config.backend,
            mcpServers: mcpServers
        )

        // `backend: "foundation-models"` runs Apple's on-device model in-process
        // instead of spawning an app-server. It is device-gated (macOS 26, Apple
        // silicon, Apple Intelligence enabled).
        //
        // A machine that cannot honour it is a hard error, never a fallback.
        // Silently starting a different backend than the config named turns a
        // misconfiguration into a mystery — the session runs, answers
        // differently, and the only clue is a warning line scrolled off above
        // the banner. Failing here says exactly what is wrong and what to change.
        if Self.wantsFoundationModels(config.backend) {
            #if canImport(FoundationModels)
            if #available(macOS 26.0, *) {
                self.backend = try await MainActor.run {
                    try FoundationModelsBackend.make(perception: ScreenPerception())
                }
                logger.info("Backend: foundation-models (on-device, in-process)")
            } else {
                throw AgentSessionError(
                    "Cannot start the foundation-models backend: it needs macOS 26 or later, "
                        + "and this system is older. To use a different backend, set `backend:` "
                        + "in the config (e.g. \"gallium\" or \"codex\") or export "
                        + "VOICE_AGENT_BACKEND."
                )
            }
            #else
            throw AgentSessionError(
                "Cannot start the foundation-models backend: this build has no "
                    + "FoundationModels framework. To use a different backend, set `backend:` "
                    + "in the config (e.g. \"gallium\" or \"codex\") or export "
                    + "VOICE_AGENT_BACKEND."
            )
            #endif
        } else {
            let agent = try agentNew(config: agentConfig, approver: approver)
            self.backend = AppServerBackend(
                agent: agent,
                program: Self.effectiveBackendProgram(config.backend),
                model: config.llm.model ?? config.llm.modelPath
            )
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
                await backend.setSystemPrompt(systemPrompt)
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
            await backend.addSkill(name: skill.name, description: skill.description, prompt: skill.prompt)
        }
        logger.info("Skills registered (\(discoveredSkills.count) from \(skillPaths))")
    }

    // MARK: - Lifecycle

    /// The program Rust will spawn, mirroring `resolve_backend`'s precedence:
    /// `VOICE_AGENT_BACKEND` → config `backend:` → `gallium`. For display only —
    /// Rust does the real resolution and logs which source won.
    static func effectiveBackendProgram(_ configured: String?) -> String {
        resolveBackendProgram(
            env: ProcessInfo.processInfo.environment["VOICE_AGENT_BACKEND"],
            configured: configured
        )
    }

    /// The pure half, so the precedence can be tested without mutating
    /// process-global environment from parallel tests — the same split as
    /// `resolve_backend` on the Rust side.
    static func resolveBackendProgram(env: String?, configured: String?) -> String {
        // A request for the in-process backend that reached here means it was
        // unavailable and we fell back, so it never names the program.
        let candidates = [env, wantsFoundationModels(configured) ? nil : configured]
        for candidate in candidates {
            if let spec = candidate?.trimmingCharacters(in: .whitespaces), !spec.isEmpty {
                return spec.split(separator: " ").first.map(String.init) ?? "gallium"
            }
        }
        return "gallium"
    }

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
    public func step(_ text: String) async throws -> AgentCore.AgentResponse {
        try await backend.step(text)
    }

    /// Reset conversation history.
    public func reset() async {
        await backend.reset()
    }

    /// The conversation so far, formatted for display.
    public func conversationHistory() async -> String {
        await backend.conversationHistory()
    }

    /// Process a slash command. Returns true if handled.
    public func handleCommand(_ command: String) async -> Bool {
        switch command {
        case "/reset":
            await backend.reset()
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
