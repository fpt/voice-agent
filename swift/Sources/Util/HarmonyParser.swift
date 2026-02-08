import Foundation

/// Parses Harmony template output to extract the final response
public struct HarmonyParser {
    /// Extracts content from the final channel in Harmony format
    /// Example input: "<|channel|>analysis<|message|>...<|channel|>final<|message|>Hello!"
    /// Returns: "Hello!"
    public static func extractFinalResponse(_ harmonyOutput: String) -> String {
        // Look for the final channel marker
        let finalMarker = "<|channel|>final<|message|>"

        guard let finalRange = harmonyOutput.range(of: finalMarker) else {
            // If no final marker found, return the original output
            return harmonyOutput
        }

        // Extract everything after the final marker
        let startIndex = finalRange.upperBound
        var result = String(harmonyOutput[startIndex...])

        // Remove any trailing control tokens like <|end|>
        let controlTokens = ["<|end|>", "<|start|>"]
        for token in controlTokens {
            if let tokenRange = result.range(of: token) {
                result = String(result[..<tokenRange.lowerBound])
            }
        }

        return result.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Extracts content from the analysis channel (useful for debugging)
    public static func extractAnalysis(_ harmonyOutput: String) -> String? {
        let analysisMarker = "<|channel|>analysis<|message|>"
        let finalMarker = "<|channel|>final"

        guard let analysisRange = harmonyOutput.range(of: analysisMarker) else {
            return nil
        }

        let startIndex = analysisRange.upperBound

        // Find where analysis ends (either at final marker or end of string)
        if let finalRange = harmonyOutput.range(of: finalMarker) {
            let endIndex = finalRange.lowerBound
            return String(harmonyOutput[startIndex..<endIndex])
                .trimmingCharacters(in: .whitespacesAndNewlines)
        } else {
            return String(harmonyOutput[startIndex...])
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }
    }
}
