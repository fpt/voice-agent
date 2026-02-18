import Foundation
import CoreGraphics
import Util
import AgentBridge
import AgentKit
import CEditline
import TTS
import Audio
import ScreenCapture

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
    print("Assistant: \(text)\n")
    // Only TTS high-priority (user speech) responses.
    // Normal-priority (watcher summaries) are print-only to avoid mic feedback loop.
    if ttsEnabled && priority == .high {
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

// Periodic window list (every 30s) -> situation message
let wm = WindowManager()
let windowListPoller = Task { @MainActor in
    while !Task.isCancelled {
        if let list = try? await wm.listWindows() {
            let text = list.map { $0.summary }.joined(separator: "\n")
            session.agent.pushSituationMessage(
                text: "[screen] Windows:\n\(text)", source: "screen", sessionId: ""
            )
        }
        try? await Task.sleep(for: .seconds(30))
    }
}

// Capture request fulfillment (100ms polling)
let capturePoller = Task { @MainActor in
    var lastCapturedImage: CGImage? = nil
    var lastCaptureInfo: WindowInfo? = nil

    while !Task.isCancelled {
        try? await Task.sleep(for: .milliseconds(100))
        let requests = session.agent.drainCaptureRequests()
        for req in requests {
            // find_window: keyword search, return matching windows as text
            if let keywords = req.searchKeywords, !keywords.isEmpty {
                do {
                    let allWindows = try await wm.listWindows()
                    let kws = keywords.lowercased().split(separator: " ").map(String.init)
                    let matched = allWindows.filter { win in
                        let haystack = "\(win.title ?? "") \(win.appName ?? "")".lowercased()
                        return kws.allSatisfy { haystack.contains($0) }
                    }
                    let text: String
                    if matched.isEmpty {
                        let all = allWindows.map { $0.summary }.joined(separator: "\n  ")
                        text = "No windows matched keywords: \(keywords)\n\nAll windows:\n  \(all)"
                    } else {
                        let lines = matched.map { $0.summary }.joined(separator: "\n  ")
                        text = "Found \(matched.count) window(s):\n  \(lines)"
                    }
                    session.agent.submitCaptureResult(id: req.id, imageBase64: "", metadataJson: text)
                } catch {
                    session.agent.submitCaptureResult(
                        id: req.id, imageBase64: "", metadataJson: "Error: \(error)"
                    )
                }
                continue
            }

            do {
                let hasCrop = req.cropX != nil || req.cropY != nil
                    || req.cropW != nil || req.cropH != nil
                let isCapture = (req.windowName != nil && !req.windowName!.isEmpty)
                    || (req.processName != nil && !req.processName!.isEmpty)
                let isOcr = req.ocr == true
                let isDetect = req.detect == true

                var image: CGImage
                var info: WindowInfo

                if isCapture {
                    // Capture a new window
                    if let name = req.windowName, !name.isEmpty {
                        (image, info) = try await wm.captureByTitle(name)
                    } else {
                        (image, info) = try await wm.captureByProcess(req.processName!)
                    }
                    // Cache the full image
                    lastCapturedImage = image
                    lastCaptureInfo = info
                } else if let cached = lastCapturedImage, let cachedInfo = lastCaptureInfo {
                    // Use cached image for crop/OCR/detect
                    image = cached
                    info = cachedInfo
                } else {
                    session.agent.submitCaptureResult(
                        id: req.id, imageBase64: "",
                        metadataJson: "Error: no window specified and no cached image"
                    )
                    continue
                }

                // Apply crop if requested
                var cropLabel = ""
                if hasCrop {
                    let cx = req.cropX ?? 0.0
                    let cy = req.cropY ?? 0.0
                    let cw = req.cropW ?? 1.0
                    let ch = req.cropH ?? 1.0
                    if let cropped = WindowManager.cropCGImage(image, x: cx, y: cy, w: cw, h: ch) {
                        image = cropped
                        cropLabel = ", Cropped: \(cx),\(cy) \(Int(cw * 100))%x\(Int(ch * 100))%"
                    }
                }

                if isOcr || isDetect {
                    // Text-only mode: OCR and/or object detection
                    var parts: [String] = []
                    let header = "Window: \(info.title ?? "?"), App: \(info.appName ?? "?")\(cropLabel)"
                    parts.append(header)
                    if isOcr {
                        let entries = try performOCR(on: image)
                        parts.append(formatOCRResults(entries))
                    }
                    if isDetect {
                        let objects = try performObjectDetection(on: image)
                        parts.append(formatDetectionResults(objects))
                    }
                    let metadata = parts.joined(separator: "\n")
                    session.agent.submitCaptureResult(id: req.id, imageBase64: "", metadataJson: metadata)
                } else {
                    // Image mode: return screenshot
                    let base64 = WindowManager.cgImageToBase64(image) ?? ""
                    let metadata = "Window: \(info.title ?? "?"), App: \(info.appName ?? "?"), Size: \(Int(info.frame.width))x\(Int(info.frame.height))\(cropLabel)"
                    session.agent.submitCaptureResult(id: req.id, imageBase64: base64, metadataJson: metadata)
                }
            } catch {
                // On failure, include the list of available windows so the LLM can retry
                var msg = "Error: \(error)"
                if let list = try? await wm.listWindows(), !list.isEmpty {
                    let windowList = list.map { $0.summary }.joined(separator: "\n  ")
                    msg += "\n\nAvailable windows:\n  \(windowList)"
                }
                session.agent.submitCaptureResult(
                    id: req.id, imageBase64: "", metadataJson: msg
                )
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

// Cleanup
session.stop()
windowListPoller.cancel()
capturePoller.cancel()

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
