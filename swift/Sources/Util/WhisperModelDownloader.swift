import Foundation

/// Downloads Whisper GGML models from HuggingFace if not already cached.
/// Models are stored at ~/.cache/whisper/ggml-{model}.bin
public struct WhisperModelDownloader {
    private static let logger = Logger("WhisperModel")
    private static let cacheDir = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".cache/whisper")
    private static let baseURL = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

    /// Valid model names (from whisper.cpp)
    private static let validModels: Set<String> = [
        "tiny", "tiny.en", "tiny-q5_1", "tiny.en-q5_1", "tiny-q8_0",
        "base", "base.en", "base-q5_1", "base.en-q5_1", "base-q8_0",
        "small", "small.en", "small.en-tdrz", "small-q5_1", "small.en-q5_1", "small-q8_0",
        "medium", "medium.en", "medium-q5_0", "medium.en-q5_0", "medium-q8_0",
        "large-v1", "large-v2", "large-v2-q5_0", "large-v2-q8_0",
        "large-v3", "large-v3-q5_0", "large-v3-turbo", "large-v3-turbo-q5_0", "large-v3-turbo-q8_0",
    ]

    /// Resolve a model name to a local file path, downloading if necessary.
    /// Returns the absolute path to the GGML model file.
    public static func resolve(model: String) async throws -> String {
        guard validModels.contains(model) else {
            throw WhisperModelError.invalidModel(model, Array(validModels.sorted()))
        }

        let filename = "ggml-\(model).bin"
        let destination = cacheDir.appendingPathComponent(filename)

        if FileManager.default.fileExists(atPath: destination.path) {
            logger.info("Model '\(model)' found at \(destination.path)")
            return destination.path
        }

        // Create cache directory
        try FileManager.default.createDirectory(at: cacheDir, withIntermediateDirectories: true)

        let url = URL(string: "\(baseURL)/\(filename)")!
        logger.info("Downloading model '\(model)' from \(url)...")
        print("Downloading Whisper model '\(model)'... (this is a one-time download)")

        let (tempURL, response) = try await URLSession.shared.download(from: url)

        guard let httpResponse = response as? HTTPURLResponse,
              httpResponse.statusCode == 200 else {
            let code = (response as? HTTPURLResponse)?.statusCode ?? -1
            throw WhisperModelError.downloadFailed(model, code)
        }

        // Move to final location
        // Remove any existing partial file
        try? FileManager.default.removeItem(at: destination)
        try FileManager.default.moveItem(at: tempURL, to: destination)

        let fileSize = try FileManager.default.attributesOfItem(atPath: destination.path)[.size] as? Int64 ?? 0
        let sizeMB = Double(fileSize) / 1_000_000.0
        logger.info("Model '\(model)' downloaded (\(String(format: "%.1f", sizeMB)) MB)")
        print("Model '\(model)' ready (\(String(format: "%.0f", sizeMB)) MB)")

        return destination.path
    }

    /// Return the expected path for a model name (without downloading).
    public static func modelPath(for model: String) -> String {
        cacheDir.appendingPathComponent("ggml-\(model).bin").path
    }
}

public enum WhisperModelError: Error, CustomStringConvertible {
    case invalidModel(String, [String])
    case downloadFailed(String, Int)

    public var description: String {
        switch self {
        case .invalidModel(let name, let valid):
            return "Invalid Whisper model '\(name)'. Valid models: \(valid.joined(separator: ", "))"
        case .downloadFailed(let name, let code):
            return "Failed to download model '\(name)' (HTTP \(code))"
        }
    }
}
