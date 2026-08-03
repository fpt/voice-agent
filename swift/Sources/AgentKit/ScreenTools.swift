import Foundation
import ScreenCapture
import Util

/// Screen-awareness tools executed **in Swift**, for a backend whose tools run
/// in-process.
///
/// The app-server path cannot use these: its tools live in Rust, which reaches
/// these same APIs through the capture bridge in the frontend instead. Here
/// there is no bridge and no poller — the tool calls `WindowManager` directly.
///
/// Everything returns text. Nothing returns an image: the on-device model has a
/// 4096-token window shared between input and output, so a screenshot is not
/// something it can afford. See docs/FOUNDATION_MODELS.md.
@MainActor
public final class ScreenTools {

    /// Hard cap on any single tool result, in characters.
    ///
    /// ~4 chars/token puts this at roughly 375 tokens — under 10% of the
    /// window. Screen OCR routinely runs to tens of thousands of characters and
    /// would blow the whole budget in one call, and Foundation Models *throws*
    /// on overflow rather than truncating. The equivalent cap on the Rust side
    /// (`MAX_OUTPUT_CHARS`) was removed in #7 because its only caller was dead;
    /// this is the same idea, applied where it actually runs.
    public static let maxResultChars = 1500

    private let windows = WindowManager()
    private let logger = Logger("ScreenTools")

    public init() {}

    /// Truncate to ``maxResultChars`` on a character boundary, saying so.
    public static func cap(_ text: String) -> String {
        guard text.count > maxResultChars else { return text }
        let kept = String(text.prefix(maxResultChars))
        return "\(kept)\n… (truncated: \(kept.count)/\(text.count) chars — narrow the query)"
    }

    /// The user's real windows, system/incognito noise filtered out.
    public func listWindows() async throws -> String {
        let list = try await windows.listWindows(excludeNoise: true)
        guard !list.isEmpty else { return "No user windows found." }
        let lines = list.map { $0.findWindowDescription }.joined(separator: "\n  ")
        return Self.cap("Open windows (\(list.count)):\n  \(lines)")
    }

    /// Windows whose title or process matches `keywords`.
    public func findWindow(keywords: String) async throws -> String {
        let list = try await windows.listWindows()
        let needle = keywords.lowercased()
        let hits = list.filter {
            $0.findWindowDescription.lowercased().contains(needle)
        }
        guard !hits.isEmpty else { return "No window matching '\(keywords)'." }
        let lines = hits.map { $0.findWindowDescription }.joined(separator: "\n  ")
        return Self.cap("Matching windows (\(hits.count)):\n  \(lines)")
    }

    /// Text read off a window via OCR. Capped hard — this is the call most
    /// likely to overrun the context window.
    public func readWindow(titleOrProcess: String) async throws -> String {
        let (image, info) = try await windows.captureByTitle(titleOrProcess)
        let entries = try performOCR(on: image)
        let text = formatOCRResults(entries)
        guard !text.isEmpty else { return "No text found in '\(info.title)'." }
        return Self.cap("Text in '\(info.title)':\n\(text)")
    }
}
