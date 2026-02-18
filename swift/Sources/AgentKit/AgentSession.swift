import Foundation
import AgentBridge
import Util
import TTS
import Watcher

/// Shared agent lifecycle — usable from CLI, iOS, or any other frontend.
public class AgentSession: @unchecked Sendable {

    // MARK: - Public properties

    public let agent: Agent
    public let tts: TextToSpeech
    public let config: Config
    public let language: String
    public let configPath: String

    /// Called when the summary poller produces a response (high = user speech, normal = watcher).
    /// Parameters: response text, priority.
    public var onResponse: (@Sendable (String, EventPriority) -> Void)?

    /// Called when a watcher summary is received (before LLM processing).
    public var onWatcherSummary: (@Sendable (String) -> Void)?

    /// Called when the summary poller encounters an error.
    public var onError: (@Sendable (Error) -> Void)?

    // MARK: - Private state

    private let logger = Logger("AgentSession")
    private var agentLock = os_unfair_lock()
    private var summaryPoller: Task<Void, Never>?
    private var socketReceiver: SocketReceiver?
    private var sessionWatcher: SessionJSONLWatcher?

    // MARK: - Init

    /// Initialize agent, TTS, and load skills.
    /// Does NOT start watcher or poller — call `start()` for that.
    public init(config: Config, configPath: String) async throws {
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

        // Resolve model path (relative to config dir or absolute)
        var modelPath: String? = nil
        if let cfgModelPath = config.llm.modelPath {
            if cfgModelPath.hasPrefix("/") {
                modelPath = cfgModelPath
            } else {
                let configDir = URL(fileURLWithPath: configPath).deletingLastPathComponent()
                modelPath = configDir.appendingPathComponent(cfgModelPath).path
            }

            // Auto-download if model file is missing
            if let path = modelPath, !FileManager.default.fileExists(atPath: path),
               let repo = config.llm.modelRepo, let file = config.llm.modelFile {
                modelPath = try await ModelDownloader.ensureModel(path: path, repo: repo, file: file)
            }
        }

        let agentConfig = AgentConfig(
            modelPath: modelPath,
            baseUrl: config.llm.baseURL,
            model: config.llm.model,
            apiKey: apiKey,
            useHarmonyTemplate: config.llm.harmonyTemplate,
            temperature: config.llm.temperature,
            maxTokens: UInt32(config.llm.maxTokens),
            language: language,
            workingDir: FileManager.default.currentDirectoryPath,
            reasoningEffort: config.llm.reasoningEffort,
            watcherDebounceSecs: (config.watcher?.enabled == true)
                ? config.watcher?.debounceInterval ?? 3.0
                : nil
        )

        self.agent = try agentNew(config: agentConfig)
        logger.info("Agent initialized")

        // TTS (must init before post-init setup that captures self)
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

        // --- Post-init setup (all stored properties initialized) ---

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
                agent.setSystemPrompt(prompt: systemPrompt)
                logger.info("Loaded system prompt from \(resolvedPath)")
            } catch {
                logger.warning("Failed to load system prompt: \(error)")
            }
        }

        // Load skills
        let projectDir = URL(fileURLWithPath: configPath).deletingLastPathComponent().path
        let discoveredSkills = SkillLoader.loadAll(projectDir: projectDir)
        for skill in discoveredSkills {
            agent.addSkill(name: skill.name, description: skill.description, prompt: skill.prompt)
        }
        logger.info("Skills registered (\(discoveredSkills.count) from skills/ and ~/.claude)")
    }

    // MARK: - Lifecycle

    /// Start watcher event sources and summary poller.
    public func start() {
        startWatcher()
        startSummaryPoller()
    }

    /// Stop all background tasks and watcher resources.
    public func stop() {
        summaryPoller?.cancel()
        summaryPoller = nil
        sessionWatcher?.stop()
        sessionWatcher = nil
        socketReceiver?.stop()
        socketReceiver = nil
    }

    // MARK: - Agent calls (thread-safe)

    /// Run agent.step() with lock serialization.
    public func step(_ text: String) throws -> AgentResponse {
        try locked { try agent.step(userInput: text) }
    }

    /// Run agent.chatOnce() with lock serialization.
    public func chatOnce(input: String, skillName: String?) throws -> String {
        try locked { try agent.chatOnce(input: input, skillName: skillName) }
    }

    /// Reset conversation history.
    public func reset() {
        agent.reset()
    }

    /// Process a slash command. Returns true if handled.
    public func handleCommand(_ command: String) -> Bool {
        switch command {
        case "/reset":
            agent.reset()
            return true
        case "/history":
            return true  // caller should print agent.getConversationHistory()
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

    /// Format response text (strip Harmony wrapper if needed).
    public func formatResponse(_ text: String) -> String {
        config.llm.harmonyTemplate
            ? HarmonyParser.extractFinalResponse(text)
            : text
    }

    // MARK: - Private

    private func locked<T>(_ body: () throws -> T) rethrows -> T {
        os_unfair_lock_lock(&agentLock)
        defer { os_unfair_lock_unlock(&agentLock) }
        return try body()
    }

    private func startWatcher() {
        guard let wc = config.watcher, wc.enabled else { return }

        // Session JSONL watcher
        let sessionPath = wc.sessionPath ?? SessionJSONLWatcher.findActiveSessionJSONL()
        if let sp = sessionPath {
            logger.info("Watching session JSONL: \(sp)")
            let watcher = SessionJSONLWatcher(filePath: sp)
            sessionWatcher = watcher
            Task.detached { [agent] in
                for await event in watcher.events() {
                    if let json = event.toRouterJSON() {
                        try? agent.feedWatcherEvent(json: json)
                    }
                }
            }
        } else {
            logger.warning("No active session JSONL found to watch")
        }

        // Socket receiver
        let sockPath = wc.socketPath ?? "/tmp/voice-agent-\(getuid()).sock"
        let receiver = SocketReceiver(socketPath: sockPath)
        socketReceiver = receiver
        do {
            try receiver.start()
            logger.info("Socket receiver listening on \(sockPath)")
            Task.detached { [agent] in
                for await event in receiver.events() {
                    if let json = event.toRouterJSON() {
                        try? agent.feedWatcherEvent(json: json)
                    }
                }
            }
        } catch {
            logger.error("Failed to start socket receiver: \(error)")
        }
    }

    private func startSummaryPoller() {
        summaryPoller = Task.detached { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(100))
                guard let self else { return }

                let summaries = self.agent.drainWatcherSummaries()
                guard !summaries.isEmpty else { continue }

                // Process high-priority (user speech) first so watcher events
                // don't block/delay the user's response.
                let high = summaries.filter { $0.priority == .high }
                let normal = summaries.filter { $0.priority != .high }

                for summary in high {
                    do {
                        let response = try self.step(summary.text)
                        let text = self.formatResponse(response.content)
                        self.onResponse?(text, summary.priority)
                    } catch {
                        self.onError?(error)
                    }
                }

                for summary in normal {
                    do {
                        self.onWatcherSummary?(summary.text)
                        let response = try self.chatOnce(
                            input: "[System Event] \(summary.text)",
                            skillName: "claude-activity-report"
                        )
                        let text = self.formatResponse(response)
                        // Normal priority: print only, no TTS (avoids mic feedback loop)
                        self.onResponse?(text, summary.priority)
                    } catch {
                        self.onError?(error)
                    }
                }
            }
        }
    }
}
