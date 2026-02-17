import Foundation
import Util
import AgentBridge
import AgentKit
import CEditline
import TTS
import Audio

let logger = Logger("Main")

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

// Initialize AgentSession (agent + TTS + skills)
let session: AgentSession
do {
    session = try await AgentSession(config: config, configPath: configPath)
} catch {
    logger.error("Failed to initialize agent: \(error)")
    exit(1)
}

let ttsEnabled = config.tts?.enabled ?? false

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

// Wire AgentSession callbacks for CLI output
session.onResponse = { @Sendable text, priority in
    if priority == .high {
        print("Assistant: \(text)\n")
    } else {
        print("Assistant: \(text)\n")
    }
    if ttsEnabled {
        Task { @MainActor in
            audioCapture.mute()
            await session.tts.speakAsync(text)
            audioCapture.unmute()
        }
    }
}

session.onWatcherSummary = { @Sendable text in
    print("\n\u{1B}[36m[Watcher]\u{1B}[0m \(text)\n")
}

session.onError = { @Sendable error in
    logger.error("Summary processing error: \(error)")
}

// Start watcher + summary poller
session.start()

// Route to voice or text mode
if sttConfig.enabled {
    await runContinuousVoiceMode()
} else {
    await runTextMode()
}

// Cleanup
session.stop()

// Skip C++ static destructors to avoid ggml Metal device assertion crash.
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
            handleCommand(userInput)
            continue
        }

        do {
            let response = try session.step(userInput)
            let finalResponse = session.formatResponse(response.content)

            if let reasoning = response.reasoning {
                print("\u{1B}[90m💭 \(reasoning)\u{1B}[0m\n")
            }
            print("Assistant: \(finalResponse)\n")

            if ttsEnabled {
                await session.tts.speakAsync(finalResponse)
            }

            turnCount += 1
        } catch {
            logger.error("Agent error: \(error)")
            print("Error: \(error)\n")
        }
    }
}

func handleCommand(_ command: String) {
    switch command {
    case "/quit", "/exit":
        session.tts.stop()
        print("Goodbye!")
        _exit(0)
    case "/help":
        printHelp()
    case "/history":
        print("Conversation History:")
        print(session.agent.getConversationHistory())
        print()
    case "/reset":
        session.reset()
        print("Conversation history cleared.\n")
    case "/voices":
        TextToSpeech.printAvailableVoices()
    case "/stop":
        if session.tts.speaking { session.tts.stop(); print("TTS stopped.\n") }
        else { print("TTS is not currently speaking.\n") }
    default:
        if !session.handleCommand(command) {
            print("Unknown command: \(command)")
            print("Type /help for available commands.\n")
        }
    }
}

// MARK: - Continuous Voice Mode

func runContinuousVoiceMode() async {
    let combineWindowMs = 500
    let micMuteDurationSecs: Double = 3.0
    var bufferedVoice: String? = nil
    var combineTimer: Task<Void, Never>? = nil
    var micUnmuteTask: Task<Void, Never>? = nil

    let voiceQueue = VoiceQueue()

    func muteMicWithTimer() {
        audioCapture.mute()
        micUnmuteTask?.cancel()
        micUnmuteTask = Task { @MainActor in
            try? await Task.sleep(for: .seconds(micMuteDurationSecs))
            guard !Task.isCancelled else { return }
            if !session.tts.speaking {
                audioCapture.unmute()
            }
        }
    }

    func feedInput(voiceText: String?, typedText: String?) {
        var parts: [String] = []
        if let v = voiceText { parts.append(v) }
        if let t = typedText { parts.append("----text: \(t)") }
        let combined = parts.joined(separator: "\n")
        guard !combined.isEmpty else { return }
        session.agent.feedUserSpeech(text: combined)
    }

    audioCapture.onVolatileResult = { text in
        print("\r\u{1B}[K  \(text)", terminator: "")
        fflush(stdout)
    }

    audioCapture.onFinalResult = { text in
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        print("\r\u{1B}[KYou (voice): \(trimmed)\n")
        voiceQueue.enqueue(trimmed)
    }

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

        _rlLineReady = false
        _rlCompletedLine = nil
        _rlGotEOF = false

        let stdinReader = Task.detached {
            rl_callback_handler_install("> ", rlLineCallback)

            while !Task.isCancelled && !_rlGotEOF {
                var fds = [pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0)]
                let ret = poll(&fds, 1, 50)

                if ret > 0 && (fds[0].revents & Int16(POLLIN)) != 0 {
                    await MainActor.run { muteMicWithTimer() }
                    rl_callback_read_char()
                }

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
                                handleCommand(text)
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

                if let voice = voiceQueue.dequeue() {
                    let partial: String? = {
                        guard let buf = rl_line_buffer else { return nil }
                        let s = String(cString: buf)
                        return s.isEmpty ? nil : s
                    }()

                    if let partial = partial {
                        rl_kill_text(0, rl_end)
                        rl_point = 0
                        rl_redisplay()

                        await MainActor.run {
                            print("  [+ text: \(partial)]")
                            feedInput(voiceText: voice, typedText: partial)
                        }
                    } else {
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
                                    session.agent.feedUserSpeech(text: voice)
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

        let signalSource = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
        signal(SIGINT, SIG_IGN)

        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            signalSource.setEventHandler {
                signalSource.cancel()
                continuation.resume()
            }
            signalSource.resume()
        }

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
