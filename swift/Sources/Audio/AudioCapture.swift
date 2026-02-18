import Foundation
import AVFoundation
import Speech
import Util

/// Audio capture with Apple SpeechTranscriber for live transcription
@MainActor
public class AudioCapture {
    private let logger = Logger("Audio")
    private let config: Config

    // SpeechAnalyzer / SpeechTranscriber
    private var transcriber: SpeechTranscriber?
    private var analyzer: SpeechAnalyzer?
    private var analyzerFormat: AVAudioFormat?

    // Mic capture
    private let audioEngine = AVAudioEngine()
    nonisolated(unsafe) private var converter: AVAudioConverter?

    // Streaming
    nonisolated(unsafe) private var inputContinuation: AsyncStream<AnalyzerInput>.Continuation?
    private var resultsTask: Task<Void, any Error>?
    private var isRunning = false
    nonisolated(unsafe) private var _muted = false
    /// Incremented on each mute(). Results from before the latest mute are discarded.
    nonisolated(unsafe) private var _muteGeneration: UInt64 = 0

    // Callbacks
    public var onVolatileResult: ((String) -> Void)?
    public var onFinalResult: ((String) -> Void)?

    /// Configuration for audio capture
    public struct Config {
        public let enabled: Bool
        public let locale: Locale
        public let censor: Bool

        public init(
            enabled: Bool,
            locale: Locale = .current,
            censor: Bool = false
        ) {
            self.enabled = enabled
            self.locale = locale
            self.censor = censor
        }
    }

    public init(config: Config) {
        self.config = config
    }

    /// Initialize SpeechTranscriber and ensure model is available
    public func initialize() async throws {
        guard config.enabled else {
            logger.info("STT disabled, skipping initialization")
            return
        }

        logger.info("Initializing SpeechTranscriber (locale: \(config.locale.identifier))")

        guard SpeechTranscriber.isAvailable else {
            throw AudioCaptureError.speechTranscriberNotAvailable
        }

        // Check locale support
        let supportedLocales = await SpeechTranscriber.supportedLocales
        guard supportedLocales.contains(where: {
            $0.identifier(.bcp47) == config.locale.identifier(.bcp47)
        }) else {
            throw AudioCaptureError.unsupportedLocale(config.locale.identifier)
        }

        // Reserve locale
        for reserved in await AssetInventory.reservedLocales {
            await AssetInventory.release(reservedLocale: reserved)
        }
        try await AssetInventory.reserve(locale: config.locale)

        // Create transcriber
        let transcriber = SpeechTranscriber(
            locale: config.locale,
            transcriptionOptions: config.censor ? [.etiquetteReplacements] : [],
            reportingOptions: [.volatileResults],
            attributeOptions: []
        )
        self.transcriber = transcriber

        // Ensure model is installed
        let modules: [any SpeechModule] = [transcriber]
        let installedLocales = await SpeechTranscriber.installedLocales
        if !installedLocales.contains(where: {
            $0.identifier(.bcp47) == config.locale.identifier(.bcp47)
        }) {
            logger.info("Downloading speech model...")
            if let request = try await AssetInventory.assetInstallationRequest(supporting: modules) {
                try await request.downloadAndInstall()
            }
            logger.info("Speech model downloaded")
        }

        // Create analyzer and get best format
        let analyzer = SpeechAnalyzer(modules: modules)
        self.analyzer = analyzer
        self.analyzerFormat = await SpeechAnalyzer.bestAvailableAudioFormat(
            compatibleWith: modules
        )

        guard analyzerFormat != nil else {
            throw AudioCaptureError.noCompatibleAudioFormat
        }

        logger.info("SpeechTranscriber initialized successfully")
    }

    /// Mute audio capture (drop buffers and discard in-flight transcription results)
    public func mute() {
        _muted = true
        _muteGeneration &+= 1
        logger.debug("Audio capture muted (gen \(_muteGeneration))")
    }

    /// Unmute audio capture.
    /// Bumps generation again so in-flight results from the muted period are discarded.
    public func unmute() {
        _muteGeneration &+= 1
        _muted = false
        logger.debug("Audio capture unmuted (gen \(_muteGeneration))")
    }

    /// Request microphone permission
    public func requestMicrophonePermission() async -> Bool {
        let status = AVCaptureDevice.authorizationStatus(for: .audio)
        switch status {
        case .authorized:
            return true
        case .notDetermined:
            return await AVCaptureDevice.requestAccess(for: .audio)
        case .denied, .restricted:
            logger.error("Microphone permission denied")
            return false
        @unknown default:
            return false
        }
    }

    /// Start live transcription from microphone
    public func start() async throws {
        guard config.enabled else { return }
        guard let transcriber = transcriber, let analyzer = analyzer,
              let targetFormat = analyzerFormat else {
            throw AudioCaptureError.notInitialized
        }

        // Set up streaming input
        let (inputSequence, continuation) = AsyncStream.makeStream(of: AnalyzerInput.self)
        self.inputContinuation = continuation

        // Set up mic capture
        let inputNode = audioEngine.inputNode
        let inputFormat = inputNode.outputFormat(forBus: 0)

        guard inputFormat.sampleRate > 0 else {
            throw AudioCaptureError.permissionDenied
        }

        guard let converter = AVAudioConverter(from: inputFormat, to: targetFormat) else {
            throw AudioCaptureError.noCompatibleAudioFormat
        }
        self.converter = converter

        inputNode.installTap(onBus: 0, bufferSize: 4096, format: nil) {
            [weak self] buffer, _ in
            self?.handleAudioBuffer(buffer)
        }

        audioEngine.prepare()
        try audioEngine.start()

        // Start analyzer
        try await analyzer.start(inputSequence: inputSequence)

        // Start consuming results — discard anything delivered while muted
        // or shortly after unmute (in-flight results from pre-mute audio).
        resultsTask = Task {
            for try await result in transcriber.results {
                // Capture mute state before async dispatch to MainActor
                let muted = self._muted
                if muted { continue }

                let text = String(result.text.characters)
                let gen = self._muteGeneration
                if result.isFinal {
                    await MainActor.run {
                        // Double-check: generation unchanged means no mute/unmute happened
                        guard self._muteGeneration == gen, !self._muted else { return }
                        self.onFinalResult?(text)
                    }
                } else {
                    await MainActor.run {
                        guard self._muteGeneration == gen, !self._muted else { return }
                        self.onVolatileResult?(text)
                    }
                }
            }
        }

        isRunning = true
        logger.info("Live transcription started")
    }

    /// Stop live transcription
    public func stop() async {
        guard isRunning else { return }

        audioEngine.stop()
        audioEngine.inputNode.removeTap(onBus: 0)

        inputContinuation?.finish()
        inputContinuation = nil

        try? await analyzer?.finalizeAndFinishThroughEndOfInput()
        resultsTask?.cancel()
        resultsTask = nil

        isRunning = false
        logger.info("Live transcription stopped")
    }

    /// Handle audio buffer from mic tap
    private nonisolated func handleAudioBuffer(_ buffer: AVAudioPCMBuffer) {
        guard !_muted else { return }
        guard let converter = converter,
              let targetFormat = converter.outputFormat as AVAudioFormat? else { return }

        let frameCapacity = AVAudioFrameCount(
            ceil(Double(buffer.frameLength) * targetFormat.sampleRate / converter.inputFormat.sampleRate)
        )
        guard frameCapacity > 0,
              let convertedBuffer = AVAudioPCMBuffer(
                  pcmFormat: targetFormat, frameCapacity: frameCapacity
              ) else { return }

        var error: NSError?
        nonisolated(unsafe) var consumed = false
        nonisolated(unsafe) let sourceBuffer = buffer
        converter.convert(to: convertedBuffer, error: &error) { _, outStatus in
            if consumed {
                outStatus.pointee = .noDataNow
                return nil
            }
            consumed = true
            outStatus.pointee = .haveData
            return sourceBuffer
        }

        if error == nil, convertedBuffer.frameLength > 0 {
            inputContinuation?.yield(AnalyzerInput(buffer: convertedBuffer))
        }
    }
}

// MARK: - Errors

public enum AudioCaptureError: Error, LocalizedError {
    case notInitialized
    case permissionDenied
    case speechTranscriberNotAvailable
    case unsupportedLocale(String)
    case noCompatibleAudioFormat

    public var errorDescription: String? {
        switch self {
        case .notInitialized:
            return "AudioCapture not initialized"
        case .permissionDenied:
            return "Microphone permission denied"
        case .speechTranscriberNotAvailable:
            return "SpeechTranscriber is not available on this device"
        case .unsupportedLocale(let id):
            return "Locale \"\(id)\" is not supported for speech transcription"
        case .noCompatibleAudioFormat:
            return "No compatible audio format available"
        }
    }
}
