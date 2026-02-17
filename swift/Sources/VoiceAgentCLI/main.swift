import Foundation
import Util
import AgentBridge
import CEditline
import TTS
import Audio
import Watcher

let logger = Logger("Main")

// Serialize agent.step() calls between voice/text and watcher
nonisolated(unsafe) var _agentLockStorage = os_unfair_lock()
func agentLocked<T>(_ body: () throws -> T) rethrows -> T {
    os_unfair_lock_lock(&_agentLockStorage)
    defer { os_unfair_lock_unlock(&_agentLockStorage) }
    return try body()
}

// readline (libedit) callback globals — C callback can't capture Swift context
nonisolated(unsafe) var _rlCompletedLine: UnsafeMutablePointer<CChar>? = nil
nonisolated(unsafe) var _rlLineReady = false
nonisolated(unsafe) var _rlGotEOF = false

private func rlLineCallback(_ line: UnsafeMutablePointer<CChar>?) {
    if line != nil {
        _rlCompletedLine = line
        _rlLineReady = true
    } else {
        _rlGotEOF = true
    }
}

// Thread-safe voice queue (voice callback on MainActor -> readline thread)
final class VoiceQueue: @unchecked Sendable {
    private var queue: [String] = []
    private var lock = os_unfair_lock()

    func enqueue(_ text: String) {
        os_unfair_lock_lock(&lock)
        queue.append(text)
        os_unfair_lock_unlock(&lock)
    }

    func dequeue() -> String? {
        os_unfair_lock_lock(&lock)
        let v = queue.isEmpty ? nil : queue.removeFirst()
        os_unfair_lock_unlock(&lock)
        return v
    }
}

// Run async main
@main
struct VoiceAgentCLI {
    static func main() async {
        await runMain()
    }
}

@MainActor
func runMain() async {

// Parse command line arguments
let arguments = CommandLine.arguments
var configPath = "configs/default.yaml"

// Simple argument parsing
for (index, arg) in arguments.enumerated() {
    if arg == "--config" && index + 1 < arguments.count {
        configPath = arguments[index + 1]
    } else if arg == "--help" || arg == "-h" {
        printHelp()
        exit(0)
    } else if arg == "--verbose" || arg == "-v" {
        Logger.setLevel(.debug)
    }
}

func printHelp() {
    print("""
    Voice Agent - Local Voice Assistant

    Usage: voice-agent [OPTIONS]

    Options:
        --config PATH      Path to configuration file (default: configs/default.yaml)
        --verbose, -v      Enable verbose logging
        --help, -h         Show this help message

    Examples:
        voice-agent
        voice-agent --config custom.yaml
        voice-agent --verbose
    """)
}

// Load configuration
let config: Config
do {
    if FileManager.default.fileExists(atPath: configPath) {
        config = try Config.load(from: configPath)
        logger.info("Loaded configuration from \(configPath)")
    } else {
        config = Config.default()
        logger.warning("Config file not found, using defaults")
    }
} catch {
    logger.error("Failed to load configuration: \(error)")
    config = Config.default()
    logger.info("Using default configuration")
}

// Initialize agent
let apiKey: String? = {
    if let envKey = ProcessInfo.processInfo.environment["OPENAI_API_KEY"], !envKey.isEmpty {
        logger.info("Using OPENAI_API_KEY from environment variable")
        return envKey
    } else if let configKey = config.llm.apiKey, !configKey.isEmpty {
        logger.info("Using API key from configuration file")
        return configKey
    } else {
        logger.info("No API key provided (using local provider)")
        return nil
    }
}()

let language = config.agent.language ?? "en"

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
        do {
            modelPath = try await ModelDownloader.ensureModel(path: path, repo: repo, file: file)
        } catch {
            logger.error("Failed to download model: \(error)")
            exit(1)
        }
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

let agent: Agent
do {
    agent = try agentNew(config: agentConfig)
    logger.info("Agent initialized successfully")

    if let systemPromptPath = config.agent.systemPromptPath {
        do {
            var resolvedPath = systemPromptPath
            if !systemPromptPath.hasPrefix("/") {
                let configURL = URL(fileURLWithPath: configPath)
                let configDir = configURL.deletingLastPathComponent()
                resolvedPath = configDir.appendingPathComponent(systemPromptPath).path
            }
            var systemPrompt = try String(contentsOfFile: resolvedPath, encoding: .utf8)

            // Replace template variables
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
} catch {
    logger.error("Failed to initialize agent: \(error)")
    exit(1)
}

// Load skills from skills/ directory and ~/.claude/plugins
let projectDir = URL(fileURLWithPath: configPath).deletingLastPathComponent().path
let discoveredSkills = SkillLoader.loadAll(projectDir: projectDir)
for skill in discoveredSkills {
    agent.addSkill(name: skill.name, description: skill.description, prompt: skill.prompt)
}
logger.info("Skills registered (\(discoveredSkills.count) from skills/ and ~/.claude)")

// Initialize TTS
let ttsConfig = config.tts ?? Config.TTSConfig(
    enabled: false,
    voice: nil,
    rate: 0.5,
    pitchMultiplier: 1.0,
    volume: 1.0
)

let ttsVoice: String? = ttsConfig.voice ?? {
    switch language {
    case "ja": return "com.apple.voice.enhanced.ja-JP.Kyoko"
    default: return "com.apple.voice.enhanced.en-US.Samantha"
    }
}()

let tts = TextToSpeech(config: TextToSpeech.Config(
    enabled: ttsConfig.enabled,
    voice: ttsVoice,
    rate: ttsConfig.rate,
    pitchMultiplier: ttsConfig.pitchMultiplier,
    volume: ttsConfig.volume
))

// Initialize STT with SpeechTranscriber
let sttConfig = config.stt ?? Config.STTConfig(enabled: false)

let locale: Locale = {
    if let id = sttConfig.locale {
        return Locale(identifier: id)
    }
    return Locale.current
}()

let audioCapture = AudioCapture(config: AudioCapture.Config(
    enabled: sttConfig.enabled,
    locale: locale,
    censor: sttConfig.censor ?? false
))

if sttConfig.enabled {
    logger.info("Initializing SpeechTranscriber...")
    do {
        try await audioCapture.initialize()
        logger.info("SpeechTranscriber initialized successfully")
    } catch {
        logger.error("Failed to initialize SpeechTranscriber: \(error)")
        logger.info("Continuing without STT")
    }
}

// Initialize watcher event sources (SocketReceiver + SessionJSONLWatcher feed into Rust EventRouter)
var sessionWatcher: SessionJSONLWatcher?
var socketReceiver: SocketReceiver?

if let wc = config.watcher, wc.enabled {
    // Start session JSONL watcher
    let sessionPath = wc.sessionPath ?? SessionJSONLWatcher.findActiveSessionJSONL()
    if let sp = sessionPath {
        logger.info("Watching session JSONL: \(sp)")
        let watcher = SessionJSONLWatcher(filePath: sp)
        sessionWatcher = watcher
        Task.detached {
            for await event in watcher.events() {
                if let json = event.toRouterJSON() {
                    try? agent.feedWatcherEvent(json: json)
                }
            }
        }
    } else {
        logger.warning("No active session JSONL found to watch")
    }

    // Start socket receiver
    let sockPath = wc.socketPath ?? "/tmp/voice-agent-\(getuid()).sock"
    let receiver = SocketReceiver(socketPath: sockPath)
    socketReceiver = receiver
    do {
        try receiver.start()
        logger.info("Socket receiver listening on \(sockPath)")
        Task.detached {
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

// Poll Rust EventRouter for summaries (handles both user speech and watcher events)
let summaryPoller = Task.detached {
    while !Task.isCancelled {
        try? await Task.sleep(for: .milliseconds(100))
        let summaries = agent.drainWatcherSummaries()
        for summary in summaries {
            do {
                if summary.priority == .high {
                    // User speech — cancel any in-progress watcher TTS
                    if ttsConfig.enabled {
                        await MainActor.run { tts.stop(); audioCapture.mute() }
                    }
                    let response = try agentLocked {
                        try agent.step(userInput: summary.text)
                    }
                    let finalResponse = config.llm.harmonyTemplate
                        ? HarmonyParser.extractFinalResponse(response.content)
                        : response.content
                    if let reasoning = response.reasoning {
                        print("\u{1B}[90m💭 \(reasoning)\u{1B}[0m\n")
                    }
                    print("Assistant: \(finalResponse)\n")
                    if ttsConfig.enabled {
                        await tts.speakAsync(finalResponse)
                        await MainActor.run { audioCapture.unmute() }
                    }
                } else {
                    // Watcher — normal priority summary
                    logger.info("[Watcher] \(summary.text)")
                    print("\n\u{1B}[36m[Watcher]\u{1B}[0m \(summary.text)\n")
                    let response = try agentLocked {
                        try agent.chatOnce(
                            input: "[System Event] \(summary.text)",
                            skillName: "claude-activity-report"
                        )
                    }
                    let finalResponse = config.llm.harmonyTemplate
                        ? HarmonyParser.extractFinalResponse(response)
                        : response
                    print("Assistant: \(finalResponse)\n")
                    if ttsConfig.enabled {
                        await MainActor.run { audioCapture.mute() }
                        await tts.speakAsync(finalResponse)
                        await MainActor.run { audioCapture.unmute() }
                    }
                }
            } catch {
                logger.error("Summary processing error: \(error)")
            }
        }
    }
}

// Route to voice or text mode
if sttConfig.enabled {
    await runContinuousVoiceMode()
} else {
    await runTextMode()
}

// Cleanup watcher resources
summaryPoller.cancel()
sessionWatcher?.stop()
socketReceiver?.stop()

// Skip C++ static destructors to avoid ggml Metal device assertion crash.
// llama.cpp's global Metal device destructor asserts all resource sets are freed,
// but Swift/Rust object teardown hasn't completed yet during exit().
_exit(0)

// MARK: - Text Mode

func runTextMode() async {
    print("""

===========================================
  Voice Agent - Text Mode
===========================================

Model: \(config.llm.model)
Endpoint: \(config.llm.baseURL)

Type your messages below. Commands:
  /reset    - Clear conversation history
  /quit     - Exit the program
  /help     - Show this help
  /history  - Show conversation history
  /voices   - List available TTS voices
  /stop     - Stop current TTS playback

===========================================

""")

    var turnCount = 0
    let maxTurns = config.agent.maxTurns

    while turnCount < maxTurns {
        print("You: ", terminator: "")
        fflush(stdout)

        guard let line = readLine() else {
            logger.info("EOF reached, exiting")
            break
        }

        let userInput = line.trimmingCharacters(in: .whitespacesAndNewlines)
        if userInput.isEmpty { continue }

        if userInput.hasPrefix("/") {
            await handleCommand(userInput)
            continue
        }

        do {
            let response = try agentLocked {
                try agent.step(userInput: userInput)
            }
            let finalResponse = config.llm.harmonyTemplate
                ? HarmonyParser.extractFinalResponse(response.content)
                : response.content

            if let reasoning = response.reasoning {
                print("\u{1B}[90m💭 \(reasoning)\u{1B}[0m\n")
            }
            print("Assistant: \(finalResponse)\n")

            if ttsConfig.enabled {
                await tts.speakAsync(finalResponse)
            }

            turnCount += 1
        } catch {
            logger.error("Agent error: \(error)")
            print("Error: \(error)\n")
        }
    }
}

func handleCommand(_ command: String) async {
    switch command {
    case "/reset":
        agent.reset()
        print("Conversation history cleared.\n")
    case "/quit", "/exit":
        tts.stop()
        print("Goodbye!")
        _exit(0)
    case "/help":
        printHelp()
    case "/history":
        print("Conversation History:")
        print(agent.getConversationHistory())
        print()
    case "/voices":
        TextToSpeech.printAvailableVoices()
    case "/stop":
        if tts.speaking { tts.stop(); print("TTS stopped.\n") }
        else { print("TTS is not currently speaking.\n") }
    default:
        print("Unknown command: \(command)")
        print("Type /help for available commands.\n")
    }
}

// MARK: - Continuous Voice Mode

func runContinuousVoiceMode() async {
    // -- State for input combining --
    let combineWindowMs = 500
    let micMuteDurationSecs: Double = 3.0
    var bufferedVoice: String? = nil
    var combineTimer: Task<Void, Never>? = nil
    var micUnmuteTask: Task<Void, Never>? = nil

    let voiceQueue = VoiceQueue()

    // Mute mic and schedule unmute after timeout
    func muteMicWithTimer() {
        audioCapture.mute()
        micUnmuteTask?.cancel()
        micUnmuteTask = Task { @MainActor in
            try? await Task.sleep(for: .seconds(micMuteDurationSecs))
            guard !Task.isCancelled else { return }
            if !tts.speaking {
                audioCapture.unmute()
            }
        }
    }

    // Feed combined voice+text input to the agent
    func feedInput(voiceText: String?, typedText: String?) {
        var parts: [String] = []
        if let v = voiceText { parts.append(v) }
        if let t = typedText { parts.append("----text: \(t)") }
        let combined = parts.joined(separator: "\n")
        guard !combined.isEmpty else { return }
        agent.feedUserSpeech(text: combined)
    }

    // -- Set up transcription callbacks --
    audioCapture.onVolatileResult = { text in
        print("\r\u{1B}[K  \(text)", terminator: "")
        fflush(stdout)
    }

    audioCapture.onFinalResult = { text in
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        print("\r\u{1B}[KYou (voice): \(trimmed)\n")
        // Enqueue for readline thread to pick up (it checks partial text)
        voiceQueue.enqueue(trimmed)
    }

    // -- Start transcription --
    do {
        try await audioCapture.start()

        print("""

===========================================
  Voice Agent - Continuous Voice Mode
===========================================

Model: \(config.llm.model)
Endpoint: \(config.llm.baseURL)
STT: Apple SpeechTranscriber (\(locale.identifier))

Start speaking or type below. Press Ctrl+C to exit.
Commands: /reset /quit /help /history /voices /stop

===========================================

""")

        // Reset readline callback state
        _rlLineReady = false
        _rlCompletedLine = nil
        _rlGotEOF = false

        // -- Readline thread (libedit callback API + poll) --
        let stdinReader = Task.detached {
            rl_callback_handler_install("> ", rlLineCallback)

            while !Task.isCancelled && !_rlGotEOF {
                // Poll stdin with 50ms timeout
                var fds = [pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)]
                let ret = poll(&fds, 1, 50)

                if ret > 0 && (fds[0].revents & Int16(POLLIN)) != 0 {
                    // Keystroke detected — mute mic
                    await MainActor.run { muteMicWithTimer() }
                    rl_callback_read_char()
                }

                // Check for completed line (Enter pressed)
                if _rlLineReady {
                    _rlLineReady = false
                    if let cStr = _rlCompletedLine {
                        let line = String(cString: cStr)
                        if !line.isEmpty { add_history(cStr) }
                        free(cStr)
                        _rlCompletedLine = nil

                        await MainActor.run {
                            let text = line.trimmingCharacters(in: .whitespacesAndNewlines)

                            if text.hasPrefix("/") {
                                Task { @MainActor in await handleCommand(text) }
                                return
                            }

                            guard !text.isEmpty else { return }

                            let voice = bufferedVoice
                            bufferedVoice = nil
                            combineTimer?.cancel()
                            combineTimer = nil

                            if let voice = voice {
                                print("You: \(voice) + text: \(text)\n")
                            } else {
                                print("You (text): \(text)\n")
                            }
                            feedInput(voiceText: voice, typedText: text)
                        }
                    }
                }

                // Check for pending voice — combine with partial typed text
                if let voice = voiceQueue.dequeue() {
                    let partial: String? = {
                        guard let buf = rl_line_buffer else { return nil }
                        let s = String(cString: buf)
                        return s.isEmpty ? nil : s
                    }()

                    if let partial = partial {
                        // Clear readline buffer and redisplay prompt
                        rl_kill_text(0, rl_end)
                        rl_point = 0
                        rl_redisplay()

                        await MainActor.run {
                            print("  [+ text: \(partial)]")
                            feedInput(voiceText: voice, typedText: partial)
                        }
                    } else {
                        // No partial text — use combine timer
                        await MainActor.run {
                            if let existing = bufferedVoice {
                                bufferedVoice = existing + " " + voice
                            } else {
                                bufferedVoice = voice
                            }
                            combineTimer?.cancel()
                            combineTimer = Task { @MainActor in
                                try? await Task.sleep(for: .milliseconds(combineWindowMs))
                                guard !Task.isCancelled else { return }
                                if let voice = bufferedVoice {
                                    agent.feedUserSpeech(text: voice)
                                    bufferedVoice = nil
                                }
                                combineTimer = nil
                            }
                        }
                    }
                }
            }

            rl_callback_handler_remove()
        }

        // -- Wait for Ctrl+C --
        let signalSource = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
        signal(SIGINT, SIG_IGN)

        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            signalSource.setEventHandler {
                signalSource.cancel()
                continuation.resume()
            }
            signalSource.resume()
        }

        // Cleanup
        stdinReader.cancel()
        micUnmuteTask?.cancel()
        combineTimer?.cancel()
        print("\nGoodbye!")
        await audioCapture.stop()

    } catch {
        logger.error("Failed to start voice mode: \(error)")
        print("Error: \(error)\n")
    }
}

} // end runMain
