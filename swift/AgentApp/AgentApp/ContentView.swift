import SwiftUI

struct ContentView: View {
    @State private var viewModel = AgentViewModel()
    @State private var showSettings = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if !viewModel.isConfigured {
                    setupPrompt
                } else {
                    ZStack(alignment: .bottom) {
                        chatView
                        micButton
                    }
                    inputBar
                }
            }
            .navigationTitle("Agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button {
                        viewModel.reset()
                    } label: {
                        Image(systemName: "arrow.counterclockwise")
                    }
                    .disabled(viewModel.messages.isEmpty)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    HStack(spacing: 12) {
                        if viewModel.contextPercent > 0 {
                            Text("\(viewModel.contextPercent)%")
                                .font(.caption)
                                .monospacedDigit()
                                .foregroundStyle(viewModel.contextPercent >= 90 ? .red : .secondary)
                        }
                        Button {
                            showSettings = true
                        } label: {
                            Image(systemName: "gear")
                        }
                    }
                }
            }
            .sheet(isPresented: $showSettings) {
                SettingsView(viewModel: viewModel)
            }
            .onAppear {
                viewModel.initializeAgent()
            }
        }
    }

    private var setupPrompt: some View {
        VStack(spacing: 16) {
            Spacer()
            Image(systemName: viewModel.provider == .openai ? "key.fill" : "arrow.down.circle")
                .font(.system(size: 48))
                .foregroundStyle(.secondary)
            Text(viewModel.provider == .openai
                 ? "OpenAI API Key Required"
                 : "Model Download Required")
                .font(.title2)
            Text("Tap the gear icon to configure.")
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding()
    }

    private var chatView: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    ForEach(viewModel.messages) { message in
                        MessageBubble(message: message)
                            .id(message.id)
                    }
                    if viewModel.isLoading {
                        HStack {
                            ProgressView()
                                .padding(.horizontal, 16)
                                .padding(.vertical, 8)
                            Spacer()
                        }
                    }
                    if let error = viewModel.errorMessage {
                        Text(error)
                            .font(.caption)
                            .foregroundStyle(.red)
                            .padding(.horizontal)
                    }
                }
                .padding()
            }
            .onChange(of: viewModel.messages.count) {
                if let last = viewModel.messages.last {
                    withAnimation {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }

    private var inputBar: some View {
        HStack(spacing: 8) {
            if viewModel.isSpeaking {
                Button {
                    viewModel.stopSpeaking()
                } label: {
                    Image(systemName: "stop.circle.fill")
                        .font(.title2)
                        .foregroundStyle(.red)
                }
            }

            TextField("Message...", text: $viewModel.inputText, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .lineLimit(1...4)
                .onSubmit {
                    viewModel.send()
                }

            Button {
                viewModel.send()
            } label: {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.title2)
            }
            .disabled(viewModel.inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || viewModel.isLoading)
        }
        .padding(.horizontal)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private var micButton: some View {
        VStack(spacing: 4) {
            if viewModel.isListening && !viewModel.liveTranscript.isEmpty {
                Text(viewModel.liveTranscript)
                    .font(.caption)
                    .padding(8)
                    .background(.ultraThinMaterial)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .padding(.horizontal, 32)
            }

            Button {
                viewModel.toggleListening()
            } label: {
                Image(systemName: viewModel.isListening ? "mic.fill" : "mic")
                    .font(.title)
                    .foregroundStyle(.white)
                    .frame(width: 56, height: 56)
                    .background(viewModel.isListening ? Color.red : Color.blue)
                    .clipShape(Circle())
                    .shadow(radius: 4)
            }
            .disabled(viewModel.isLoading)
        }
        .padding(.bottom, 12)
    }
}

struct MessageBubble: View {
    let message: ChatMessage

    var body: some View {
        HStack {
            if message.role == .user { Spacer(minLength: 48) }

            Text(message.text)
                .padding(12)
                .background(message.role == .user ? Color.blue : Color(.systemGray5))
                .foregroundStyle(message.role == .user ? .white : .primary)
                .clipShape(RoundedRectangle(cornerRadius: 16))

            if message.role == .assistant { Spacer(minLength: 48) }
        }
    }
}

// MARK: - Settings

struct SettingsView: View {
    @Bindable var viewModel: AgentViewModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Provider") {
                    Picker("LLM Provider", selection: $viewModel.provider) {
                        ForEach(LLMProvider.allCases, id: \.self) { p in
                            Text(p.displayName).tag(p)
                        }
                    }
                    .pickerStyle(.segmented)
                }

                if viewModel.provider == .openai {
                    openAISection
                } else {
                    llamaSection
                }

                Section("Agent") {
                    Picker("Language", selection: $viewModel.language) {
                        Text("English").tag("en")
                        Text("Japanese").tag("ja")
                    }
                }

                Section("Speech") {
                    Toggle("Text-to-Speech", isOn: $viewModel.ttsEnabled)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        viewModel.initializeAgent()
                        dismiss()
                    }
                }
            }
        }
    }

    private var openAISection: some View {
        Section("OpenAI") {
            SecureField("API Key", text: $viewModel.apiKey)
                .textContentType(.password)
                .autocorrectionDisabled()
            TextField("Base URL", text: $viewModel.baseURL)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            TextField("Model", text: $viewModel.model)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
        }
    }

    private var llamaSection: some View {
        Section("Local Model") {
            modelRow
        }
    }

    @ViewBuilder
    private var modelRow: some View {
        switch viewModel.modelDownloadState {
        case .notDownloaded:
            VStack(alignment: .leading, spacing: 8) {
                Text("Qwen3-1.7B (Q8_0)")
                    .font(.headline)
                Text("1.83 GB download")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button {
                    viewModel.downloadModel()
                } label: {
                    Label("Download Model", systemImage: "arrow.down.circle.fill")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
            }
            .padding(.vertical, 4)

        case .downloading(let progress):
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("Qwen3-1.7B (Q8_0)")
                        .font(.headline)
                    Spacer()
                    Text("\(Int(progress * 100))%")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
                ProgressView(value: progress)
                    .progressViewStyle(.linear)
                HStack {
                    Text(formatBytes(Int64(progress * Double(AgentViewModel.modelSizeBytes))))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("Cancel") {
                        viewModel.cancelDownload()
                    }
                    .font(.caption)
                }
            }
            .padding(.vertical, 4)

        case .downloaded:
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                    Text("Qwen3-1.7B (Q8_0)")
                        .font(.headline)
                    Spacer()
                    Text("Ready")
                        .font(.caption)
                        .foregroundStyle(.green)
                }
                Button(role: .destructive) {
                    viewModel.deleteModel()
                } label: {
                    Label("Delete Model", systemImage: "trash")
                        .font(.caption)
                }
            }
            .padding(.vertical, 4)

        case .failed(let message):
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.red)
                    Text("Download Failed")
                        .font(.headline)
                }
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button {
                    viewModel.downloadModel()
                } label: {
                    Label("Retry", systemImage: "arrow.clockwise")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
            }
            .padding(.vertical, 4)
        }
    }

    private func formatBytes(_ bytes: Int64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: bytes)
    }
}

#Preview {
    ContentView()
}
