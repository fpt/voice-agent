import Foundation
import AVFoundation
import Speech
import Observation

/// Message in the chat history
struct ChatMessage: Identifiable {
    let id = UUID()
    let role: Role
    let text: String
    let timestamp = Date()

    enum Role {
        case user, assistant
    }
}

/// Main view model for the agent chat interface
@Observable
final class AgentViewModel {
    var messages: [ChatMessage] = []
    var inputText = ""
    var isLoading = false
    var errorMessage: String?
    var isSpeaking = false
    var isListening = false
    var liveTranscript = ""

    private var agent: Agent?
    private let synthesizer = AVSpeechSynthesizer()
    private var ttsDelegate: TTSDelegate?
    private var ttsVoice: AVSpeechSynthesisVoice?
    private var ttsRate: Float = 0.5

    // Speech recognition
    private let speechRecognizer = SFSpeechRecognizer()
    private let audioEngine = AVAudioEngine()
    private var recognitionRequest: SFSpeechAudioBufferRecognitionRequest?
    private var recognitionTask: SFSpeechRecognitionTask?

    // Config
    var apiKey: String {
        get { UserDefaults.standard.string(forKey: "openai_api_key") ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: "openai_api_key") }
    }
    var model: String {
        get { UserDefaults.standard.string(forKey: "openai_model") ?? "gpt-5-mini" }
        set { UserDefaults.standard.set(newValue, forKey: "openai_model") }
    }
    var baseURL: String {
        get { UserDefaults.standard.string(forKey: "openai_base_url") ?? "https://api.openai.com/v1" }
        set { UserDefaults.standard.set(newValue, forKey: "openai_base_url") }
    }
    var language: String {
        get { UserDefaults.standard.string(forKey: "agent_language") ?? "en" }
        set { UserDefaults.standard.set(newValue, forKey: "agent_language") }
    }
    var ttsEnabled: Bool {
        get { UserDefaults.standard.object(forKey: "tts_enabled") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "tts_enabled") }
    }

    var isConfigured: Bool {
        !apiKey.isEmpty
    }

    init() {
        ttsDelegate = TTSDelegate { [weak self] in
            self?.isSpeaking = false
        }
        synthesizer.delegate = ttsDelegate

        // Pick a default voice
        if let voice = AVSpeechSynthesisVoice(language: language == "ja" ? "ja-JP" : "en-US") {
            ttsVoice = voice
        }
    }

    func initializeAgent() {
        guard !apiKey.isEmpty else {
            errorMessage = "Please set your OpenAI API key in Settings."
            return
        }

        do {
            let languagePrompt: String = {
                switch language {
                case "ja": return "日本語で回答してください。"
                case "en": return ""
                default: return "Respond in \(language)."
                }
            }()

            var systemPrompt = "You are a kind voice agent.\nGive clear and concise response."
            if !languagePrompt.isEmpty {
                systemPrompt += "\n\(languagePrompt)"
            }

            let config = AgentConfig(
                modelPath: nil,
                baseUrl: baseURL,
                model: model,
                apiKey: apiKey,
                useHarmonyTemplate: false,
                temperature: nil,
                maxTokens: 8192,
                language: language,
                workingDir: NSHomeDirectory(),
                reasoningEffort: "medium",
                watcherDebounceSecs: nil
            )

            agent = try agentNew(config: config)
            agent?.setSystemPrompt(prompt: systemPrompt)
            errorMessage = nil
        } catch {
            errorMessage = "Failed to initialize agent: \(error.localizedDescription)"
        }
    }

    func send() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let agent else { return }

        messages.append(ChatMessage(role: .user, text: text))
        inputText = ""
        isLoading = true
        errorMessage = nil

        Task.detached { [weak self] in
            do {
                let response = try agent.step(userInput: text)
                await MainActor.run {
                    guard let self else { return }
                    self.messages.append(ChatMessage(role: .assistant, text: response.content))
                    self.isLoading = false
                    if self.ttsEnabled {
                        self.speak(response.content)
                    }
                }
            } catch {
                await MainActor.run {
                    guard let self else { return }
                    self.errorMessage = error.localizedDescription
                    self.isLoading = false
                }
            }
        }
    }

    func reset() {
        agent?.reset()
        messages.removeAll()
    }

    func stopSpeaking() {
        synthesizer.stopSpeaking(at: .immediate)
        isSpeaking = false
    }

    // MARK: - Speech Recognition

    func toggleListening() {
        if isListening {
            stopListening()
        } else {
            startListening()
        }
    }

    private func startListening() {
        // Stop TTS if playing
        if isSpeaking { stopSpeaking() }

        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            Task { @MainActor in
                guard let self else { return }
                guard status == .authorized else {
                    self.errorMessage = "Speech recognition not authorized."
                    return
                }
                self.beginRecording()
            }
        }
    }

    private func beginRecording() {
        // Cancel any previous task
        recognitionTask?.cancel()
        recognitionTask = nil

        let locale = language == "ja" ? Locale(identifier: "ja-JP") : Locale(identifier: "en-US")
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
            errorMessage = "Speech recognizer not available for \(language)."
            return
        }

        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        recognitionRequest = request

        let audioSession = AVAudioSession.sharedInstance()
        do {
            try audioSession.setCategory(.record, mode: .measurement, options: .duckOthers)
            try audioSession.setActive(true, options: .notifyOthersOnDeactivation)
        } catch {
            errorMessage = "Audio session error: \(error.localizedDescription)"
            return
        }

        let inputNode = audioEngine.inputNode
        let recordingFormat = inputNode.outputFormat(forBus: 0)
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: recordingFormat) { buffer, _ in
            request.append(buffer)
        }

        audioEngine.prepare()
        do {
            try audioEngine.start()
            isListening = true
            liveTranscript = ""
        } catch {
            errorMessage = "Audio engine error: \(error.localizedDescription)"
            return
        }

        recognitionTask = recognizer.recognitionTask(with: request) { [weak self] result, error in
            Task { @MainActor in
                guard let self else { return }
                if let result {
                    self.liveTranscript = result.bestTranscription.formattedString
                }
                if error != nil || (result?.isFinal ?? false) {
                    // Only auto-stop if final result or error; manual stop handled by stopListening
                }
            }
        }
    }

    func stopListening() {
        audioEngine.stop()
        audioEngine.inputNode.removeTap(onBus: 0)
        recognitionRequest?.endAudio()
        recognitionRequest = nil
        recognitionTask?.cancel()
        recognitionTask = nil
        isListening = false

        // Send the transcribed text
        let text = liveTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty {
            inputText = text
            liveTranscript = ""
            send()
        } else {
            liveTranscript = ""
        }
    }

    private func speak(_ text: String) {
        guard ttsEnabled, !text.isEmpty else { return }

        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = ttsVoice
        utterance.rate = ttsRate

        isSpeaking = true
        synthesizer.speak(utterance)
    }
}

/// Delegate to track TTS completion
private class TTSDelegate: NSObject, AVSpeechSynthesizerDelegate {
    let onFinish: () -> Void
    init(onFinish: @escaping () -> Void) { self.onFinish = onFinish }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        Task { @MainActor in onFinish() }
    }
    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        Task { @MainActor in onFinish() }
    }
}
