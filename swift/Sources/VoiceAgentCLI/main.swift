import Foundation
import Util
import AgentBridge
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
    reasoningEffort: config.llm.reasoningEffort
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
            let systemPrompt = try String(contentsOfFile: resolvedPath, encoding: .utf8)
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

// Register skills
agent.addSkill(
    name: "claude-activity-report",
    description: "Use when receiving [System Event] messages about Claude Code activity. Summarize what Claude Code did and provide a brief spoken update.",
    prompt: """
    You received a system event about Claude Code activity. Your task:
    1. Parse the event summary to understand what Claude Code did
    2. Provide a brief, conversational spoken response (1-2 sentences)
    3. Focus on what changed and what's most noteworthy
    4. Use natural speech patterns suitable for text-to-speech

    Examples of good responses:
    - "Claude Code just edited three files in the authentication module."
    - "Looks like the tests all passed. Twenty-one tests, zero failures."
    - "Claude Code committed a fix for the login bug."
    - "Claude Code is reading through the configuration files."
    """
)
// Load skills from ~/.claude/plugins
let discoveredSkills = SkillLoader.loadAll()
for skill in discoveredSkills {
    agent.addSkill(name: skill.name, description: skill.description, prompt: skill.prompt)
}
logger.info("Skills registered (\(1 + discoveredSkills.count) total: 1 built-in + \(discoveredSkills.count) from ~/.claude)")

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

// Initialize watcher if enabled
var sessionWatcher: SessionJSONLWatcher?
var socketReceiver: SocketReceiver?
var eventPipeline: EventPipeline?

if let wc = config.watcher, wc.enabled {
    let debounce = wc.debounceInterval ?? 3.0

    let pipeline = EventPipeline(debounceInterval: debounce) { summary in
        logger.info("[Watcher] \(summary)")
        print("\n\u{1B}[36m[Watcher]\u{1B}[0m \(summary)\n")

        do {
            let agentResponse = try agentLocked {
                try agent.stepWithAllowedTools(
                    userInput: "[System Event] \(summary)",
                    allowedTools: ["lookup_skill"]
                )
            }
            let finalResponse = config.llm.harmonyTemplate
                ? HarmonyParser.extractFinalResponse(agentResponse.content)
                : agentResponse.content

            print("Assistant: \(finalResponse)\n")

            if ttsConfig.enabled {
                await MainActor.run {
                    audioCapture.mute()
                }
                await tts.speakAsync(finalResponse)
                await MainActor.run {
                    audioCapture.unmute()
                }
            }
        } catch {
            logger.error("Watcher agent error: \(error)")
        }
    }
    eventPipeline = pipeline

    // Start session JSONL watcher
    let sessionPath = wc.sessionPath ?? SessionJSONLWatcher.findActiveSessionJSONL()
    if let sp = sessionPath {
        logger.info("Watching session JSONL: \(sp)")
        let watcher = SessionJSONLWatcher(filePath: sp)
        sessionWatcher = watcher
        Task.detached {
            for await event in watcher.events() {
                await pipeline.feed(.session(event))
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
                await pipeline.feed(.hook(event))
            }
        }
    } catch {
        logger.error("Failed to start socket receiver: \(error)")
    }
}

// Route to voice or text mode
if sttConfig.enabled {
    await runContinuousVoiceMode()
} else {
    await runTextMode()
}

// Cleanup watcher resources
sessionWatcher?.stop()
socketReceiver?.stop()
await eventPipeline?.stop()

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
        exit(0)
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
    // Set up transcription callbacks
    audioCapture.onVolatileResult = { text in
        print("\r\u{1B}[K  \(text)", terminator: "")
        fflush(stdout)
    }

    audioCapture.onFinalResult = { text in
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        print("\r\u{1B}[KYou: \(trimmed)\n")

        do {
            let response = try agentLocked {
                try agent.step(userInput: trimmed)
            }
            let finalResponse = config.llm.harmonyTemplate
                ? HarmonyParser.extractFinalResponse(response.content)
                : response.content

            if let reasoning = response.reasoning {
                print("\u{1B}[90m💭 \(reasoning)\u{1B}[0m\n")
            }
            print("Assistant: \(finalResponse)\n")

            if ttsConfig.enabled {
                Task { @MainActor in
                    audioCapture.mute()
                    await tts.speakAsync(finalResponse)
                    audioCapture.unmute()
                }
            }
        } catch {
            logger.error("Agent error: \(error)")
            print("Error: \(error)\n")
        }
    }

    // Start transcription
    do {
        try await audioCapture.start()

        let watcherStatus = config.watcher?.enabled == true
            ? "Watcher: active (socket: \(socketReceiver?.path ?? "none"))"
            : "Watcher: disabled"

        print("""

===========================================
  Voice Agent - Continuous Voice Mode
===========================================

Model: \(config.llm.model)
Endpoint: \(config.llm.baseURL)
STT: Apple SpeechTranscriber (\(locale.identifier))
\(watcherStatus)

Start speaking! Press Ctrl+C to exit.

===========================================

""")

        // Wait for Ctrl+C
        let signalSource = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
        signal(SIGINT, SIG_IGN)

        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            signalSource.setEventHandler {
                signalSource.cancel()
                continuation.resume()
            }
            signalSource.resume()
        }

        print("\nGoodbye!")
        await audioCapture.stop()

    } catch {
        logger.error("Failed to start voice mode: \(error)")
        print("Error: \(error)\n")
    }
}

} // end runMain
