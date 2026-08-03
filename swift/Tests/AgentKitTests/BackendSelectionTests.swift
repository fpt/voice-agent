import XCTest

@testable import AgentKit

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
