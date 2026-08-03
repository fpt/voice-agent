import XCTest

@testable import AgentKit

/// The tool-result cap is a hard context budget, not a hint: Foundation Models
/// throws on overflow rather than truncating, so a result that overshoots the
/// cap can fail a turn outright.
@MainActor
final class ScreenToolsCapTests: XCTestCase {

    func testShortTextPassesThroughUntouched() {
        let text = "Safari — Apple Developer Documentation"
        XCTAssertEqual(ScreenTools.cap(text), text)
    }

    func testTextExactlyAtTheCapIsNotTruncated() {
        let text = String(repeating: "x", count: ScreenTools.maxResultChars)
        XCTAssertEqual(ScreenTools.cap(text), text)
    }

    /// The regression this guards: the notice used to be appended *after* taking
    /// a full-size prefix, so every truncated result exceeded the cap.
    func testTruncatedResultHonoursTheCapIncludingItsNotice() {
        for overshoot in [1, 100, 50_000] {
            let text = String(repeating: "y", count: ScreenTools.maxResultChars + overshoot)
            let capped = ScreenTools.cap(text)
            XCTAssertLessThanOrEqual(
                capped.count, ScreenTools.maxResultChars,
                "cap overshot by \(capped.count - ScreenTools.maxResultChars) for +\(overshoot)"
            )
            XCTAssertTrue(capped.contains("truncated"), "truncation must be visible to the model")
        }
    }

    /// OCR over a full screen is the realistic case, and the one most likely to
    /// blow the window in a single tool call.
    func testOcrSizedInputIsBroughtWithinBudget() {
        let ocr = String(repeating: "The quick brown fox. ", count: 3_000)  // ~63k chars
        let capped = ScreenTools.cap(ocr)
        XCTAssertLessThanOrEqual(capped.count, ScreenTools.maxResultChars)
        // ~4 chars/token: the whole point is staying a small share of 4096.
        XCTAssertLessThan(capped.count / 4, 500)
    }
}

/// Only the in-process backend is chosen in Swift; every other value names a
/// program for the Rust side to spawn.
final class BackendSelectionTests: XCTestCase {

    func testRecognisedSpellings() {
        for name in ["foundation-models", "foundationmodels", "apple", "Foundation-Models", "  APPLE  "] {
            XCTAssertTrue(
                AgentSession.wantsFoundationModels(name),
                "\(name) should select the in-process backend"
            )
        }
    }

    func testEverythingElseGoesToTheAppServer() {
        for name in ["gallium", "codex", "my-agent --flag", "", "   "] {
            XCTAssertFalse(
                AgentSession.wantsFoundationModels(name),
                "\(name) should be left to the app-server path"
            )
        }
        XCTAssertFalse(AgentSession.wantsFoundationModels(nil))
    }
}

/// The banner used to hardcode "local (gallium backend)" whenever `baseURL` was
/// absent, so a Foundation Models session announced it was running gallium.
/// The program shown must mirror Rust's `resolve_backend` precedence.
final class BackendProgramResolutionTests: XCTestCase {

    func testConfigNamesTheProgram() {
        XCTAssertEqual(AgentSession.resolveBackendProgram(env: nil, configured: "codex"), "codex")
    }

    func testEnvBeatsTheConfig() {
        XCTAssertEqual(
            AgentSession.resolveBackendProgram(env: "codex", configured: "gallium"), "codex")
    }

    func testDefaultsToGallium() {
        XCTAssertEqual(AgentSession.resolveBackendProgram(env: nil, configured: nil), "gallium")
        XCTAssertEqual(AgentSession.resolveBackendProgram(env: "  ", configured: ""), "gallium")
    }

    /// Reaching this path with "foundation-models" configured means the device
    /// could not run it and we fell back — it never names a program to spawn.
    func testFoundationModelsFallsBackToGalliumNotItself() {
        XCTAssertEqual(
            AgentSession.resolveBackendProgram(env: nil, configured: "foundation-models"),
            "gallium"
        )
    }

    /// A spec may carry arguments; only the program is shown.
    func testOnlyTheProgramIsShown() {
        XCTAssertEqual(
            AgentSession.resolveBackendProgram(env: nil, configured: "my-agent --flag x"),
            "my-agent"
        )
    }
}
