import Foundation
import AVFoundation
import Util

/// Text-to-Speech manager using AVSpeechSynthesizer
public class TextToSpeech: NSObject, @unchecked Sendable {
    private let synthesizer: AVSpeechSynthesizer
    private let logger = Logger("TTS")
    private var isSpeaking = false
    private var completion: (() -> Void)?

    /// Configuration for TTS
    public struct Config {
        public let enabled: Bool
        public let voice: String?
        public let rate: Float
        public let pitchMultiplier: Float
        public let volume: Float

        public init(
            enabled: Bool = true,
            voice: String? = nil,
            rate: Float = 0.5,
            pitchMultiplier: Float = 1.0,
            volume: Float = 1.0
        ) {
            self.enabled = enabled
            self.voice = voice
            self.rate = rate
            self.pitchMultiplier = pitchMultiplier
            self.volume = volume
        }
    }

    private let config: Config
    private let resolvedVoice: AVSpeechSynthesisVoice?

    public init(config: Config) {
        self.config = config
        self.synthesizer = AVSpeechSynthesizer()

        // Validate voice at init time
        if let id = config.voice {
            if let voice = AVSpeechSynthesisVoice(identifier: id) {
                self.resolvedVoice = voice
            } else {
                self.resolvedVoice = nil
                // Can't use logger before super.init, print directly
                print("[TTS] ERROR: Voice '\(id)' not found on this system. Run /voices to list available voices.")
            }
        } else {
            self.resolvedVoice = AVSpeechSynthesisVoice(language: "en-US")
        }

        super.init()
        self.synthesizer.delegate = self

        if let v = resolvedVoice {
            logger.info("TTS voice: \(v.name) [\(v.identifier)]")
        }
    }

    /// Speak the given text asynchronously
    /// - Parameter text: The text to speak
    public func speakAsync(_ text: String) async {
        guard config.enabled else {
            logger.debug("TTS disabled, skipping speech")
            return
        }

        guard !text.isEmpty else {
            logger.debug("Empty text, skipping speech")
            return
        }

        // If already speaking, stop current speech
        if isSpeaking {
            logger.debug("Already speaking, stopping current speech")
            stop()
        }

        await withCheckedContinuation { continuation in
            self.completion = {
                continuation.resume()
            }
            self.isSpeaking = true

            guard let voice = self.resolvedVoice else {
                self.logger.error("No valid TTS voice configured, skipping speech")
                self.isSpeaking = false
                self.completion = nil
                continuation.resume()
                return
            }

            let utterance = AVSpeechUtterance(string: text)
            utterance.voice = voice
            utterance.rate = self.config.rate
            utterance.pitchMultiplier = self.config.pitchMultiplier
            utterance.volume = self.config.volume

            self.logger.info("Speaking: \"\(text.prefix(50))\(text.count > 50 ? "..." : "")\"")
            self.synthesizer.speak(utterance)
        }
    }

    /// Speak the given text (callback version for compatibility)
    /// - Parameters:
    ///   - text: The text to speak
    ///   - completion: Called when speech completes
    public func speak(_ text: String, completion: (() -> Void)? = nil) {
        guard config.enabled else {
            logger.debug("TTS disabled, skipping speech")
            completion?()
            return
        }

        guard !text.isEmpty else {
            logger.debug("Empty text, skipping speech")
            completion?()
            return
        }

        // If already speaking, stop current speech
        if isSpeaking {
            logger.debug("Already speaking, stopping current speech")
            stop()
        }

        guard let voice = resolvedVoice else {
            logger.error("No valid TTS voice configured, skipping speech")
            completion?()
            return
        }

        self.completion = completion
        isSpeaking = true

        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = voice
        utterance.rate = config.rate
        utterance.pitchMultiplier = config.pitchMultiplier
        utterance.volume = config.volume

        logger.info("Speaking: \"\(text.prefix(50))\(text.count > 50 ? "..." : "")\"")
        synthesizer.speak(utterance)
    }

    /// Stop current speech
    public func stop() {
        guard isSpeaking else { return }

        logger.debug("Stopping speech")
        synthesizer.stopSpeaking(at: .immediate)
        isSpeaking = false
        completion?()
        completion = nil
    }

    /// Check if currently speaking
    public var speaking: Bool {
        return isSpeaking
    }

    /// List available voices
    public static func availableVoices() -> [AVSpeechSynthesisVoice] {
        return AVSpeechSynthesisVoice.speechVoices()
    }

    /// List available voices for a specific language
    public static func availableVoices(for language: String) -> [AVSpeechSynthesisVoice] {
        return AVSpeechSynthesisVoice.speechVoices().filter { $0.language.hasPrefix(language) }
    }

    /// Print available voices (useful for debugging)
    public static func printAvailableVoices() {
        print("\nAvailable TTS Voices:")
        print("====================")

        let voices = AVSpeechSynthesisVoice.speechVoices()

        // Separate enhanced and standard voices
        let enhanced = voices.filter { $0.quality == .enhanced }
        let standard = voices.filter { $0.quality != .enhanced }

        // Print enhanced voices first
        if !enhanced.isEmpty {
            print("\n✨ Enhanced Quality Voices (Premium):")
            print("-------------------------------------")
            let groupedEnhanced = Dictionary(grouping: enhanced, by: { $0.language })
            for (language, voiceList) in groupedEnhanced.sorted(by: { $0.key < $1.key }) {
                print("\n\(language):")
                for voice in voiceList.sorted(by: { $0.name < $1.name }) {
                    print("  ✨ \(voice.name) [\(voice.identifier)]")
                }
            }
        }

        // Print standard voices
        if !standard.isEmpty {
            print("\n📢 Standard Quality Voices:")
            print("---------------------------")
            let groupedStandard = Dictionary(grouping: standard, by: { $0.language })
            for (language, voiceList) in groupedStandard.sorted(by: { $0.key < $1.key }) {
                print("\n\(language):")
                for voice in voiceList.sorted(by: { $0.name < $1.name }) {
                    print("  📢 \(voice.name) [\(voice.identifier)]")
                }
            }
        }

        print("\n💡 Tip: Use enhanced voices for best quality!")
        print("   Example: Set voice to \"com.apple.voice.enhanced.en-US.Zoe\" in config")
        print()
    }

    /// Get enhanced voices for English
    public static func enhancedEnglishVoices() -> [AVSpeechSynthesisVoice] {
        return AVSpeechSynthesisVoice.speechVoices()
            .filter { $0.language.hasPrefix("en-") && $0.quality == .enhanced }
            .sorted { $0.name < $1.name }
    }
}

// MARK: - AVSpeechSynthesizerDelegate
extension TextToSpeech: AVSpeechSynthesizerDelegate {
    public func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didStart utterance: AVSpeechUtterance) {
        logger.info("Speech started")
        print("🔊 TTS playback started")
    }

    public func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        logger.info("Speech finished")
        print("🔊 TTS playback finished")
        isSpeaking = false
        completion?()
        completion = nil
    }

    public func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didPause utterance: AVSpeechUtterance) {
        logger.debug("Speech paused")
    }

    public func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didContinue utterance: AVSpeechUtterance) {
        logger.debug("Speech continued")
    }

    public func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        logger.debug("Speech cancelled")
        isSpeaking = false
        completion?()
        completion = nil
    }
}
