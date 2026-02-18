import Foundation
import CoreGraphics
import Vision

/// A single recognized text region from OCR.
public struct OCREntry: Sendable {
    /// The recognized text string.
    public let text: String
    /// Confidence score (0.0–1.0).
    public let confidence: Float
    /// Bounding box in normalized coordinates (0.0–1.0).
    /// Origin is top-left (converted from Vision's bottom-left origin).
    public let x: Double
    public let y: Double
    public let width: Double
    public let height: Double
}

/// Perform OCR on a CGImage using Apple Vision framework.
/// Returns recognized text entries with bounding boxes.
/// Thread-safe — can be called from any thread/actor.
public func performOCR(
    on image: CGImage,
    languages: [String] = ["en-US", "ja"]
) throws -> [OCREntry] {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.recognitionLanguages = languages
    request.usesLanguageCorrection = true

    let handler = VNImageRequestHandler(cgImage: image, options: [:])
    try handler.perform([request])

    guard let observations = request.results else { return [] }

    return observations.compactMap { obs in
        guard let candidate = obs.topCandidates(1).first else { return nil }
        let box = obs.boundingBox
        // Vision uses bottom-left origin; convert to top-left
        return OCREntry(
            text: candidate.string,
            confidence: candidate.confidence,
            x: box.origin.x,
            y: 1.0 - box.origin.y - box.height,
            width: box.width,
            height: box.height
        )
    }
}

/// Format OCR entries into a human-readable string with bounding boxes.
/// The coordinates can be used directly as crop_x/y/w/h parameters.
public func formatOCRResults(_ entries: [OCREntry]) -> String {
    if entries.isEmpty {
        return "No text detected."
    }
    var lines: [String] = ["OCR Results (\(entries.count) entries):"]
    for entry in entries {
        let conf = String(format: "%.0f%%", entry.confidence * 100)
        let pos = String(format: "[%.2f,%.2f %.0f%%x%.0f%%]",
                         entry.x, entry.y, entry.width * 100, entry.height * 100)
        lines.append("  \(pos) \"\(entry.text)\" (\(conf))")
    }
    return lines.joined(separator: "\n")
}
