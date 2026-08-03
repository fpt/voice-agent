import XCTest

@testable import Util

/// Config decoding, pinned against the failures this file has actually had.
///
/// Every backend-selection and credential bug so far has been config-shaped, and
/// none of them were caught by a test because there were no Swift tests.
final class ConfigTests: XCTestCase {

    private var dir: URL!

    override func setUpWithError() throws {
        dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("voice-agent-config-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func load(_ yaml: String) throws -> Config {
        let path = dir.appendingPathComponent("config.yaml")
        try yaml.write(to: path, atomically: true, encoding: .utf8)
        return try Config.load(from: path.path)
    }

    /// The bug behind the `backend:` key existing at all: a file named
    /// `codex.yaml` used to spawn gallium, because the backend could only be
    /// chosen through an environment variable.
    func testBackendKeyIsRead() throws {
        let config = try load("""
            backend: "codex"
            llm:
              maxTokens: 8192
            agent:
              maxTurns: 50
            """)
        XCTAssertEqual(config.backend, "codex")
    }

    /// Omitting it is legal and means "use the default", not "fail to parse".
    func testBackendIsOptional() throws {
        let config = try load("""
            llm:
              maxTokens: 2048
            agent:
              maxTurns: 50
            """)
        XCTAssertNil(config.backend)
    }

    /// A codex user signed in via `codex login` sets neither. They must decode
    /// as absent rather than as empty strings — an empty `OPENAI_API_KEY` can
    /// shadow a backend's own credential fallback.
    func testCredentialsAndEndpointAreOptional() throws {
        let config = try load("""
            backend: "codex"
            llm:
              maxTokens: 8192
            agent:
              maxTurns: 50
            """)
        XCTAssertNil(config.llm.baseURL)
        XCTAssertNil(config.llm.apiKey)
        XCTAssertNil(config.llm.model)
    }

    /// Fields removed in the dead-code sweep (`harmonyTemplate`, `contextWindow`)
    /// must not break a config that still carries them. This was claimed at the
    /// time on the strength of Codable's behaviour; nothing checked it.
    func testRemovedFieldsAreIgnoredRatherThanFatal() throws {
        let config = try load("""
            backend: "gallium"
            llm:
              harmonyTemplate: true
              contextWindow: 32000
              maxTokens: 4096
              somethingInventedLater: "hello"
            agent:
              maxTurns: 50
            """)
        XCTAssertEqual(config.backend, "gallium")
        XCTAssertEqual(config.llm.maxTokens, 4096)
    }

    /// The three shipped configs must actually parse. They are the only configs
    /// most users will ever run.
    func testShippedConfigsParse() throws {
        let repo = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // UtilTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // swift
            .deletingLastPathComponent()  // repo root
        for name in ["gallium", "codex", "foundation-models"] {
            let path = repo.appendingPathComponent("configs/\(name).yaml").path
            guard FileManager.default.fileExists(atPath: path) else {
                XCTFail("configs/\(name).yaml is missing")
                continue
            }
            let config = try Config.load(from: path)
            XCTAssertNotNil(config.backend, "configs/\(name).yaml should name its backend")
        }
    }

    /// `foundation-models.yaml` names an in-process backend, not a program on
    /// PATH — the distinction the Swift side switches on.
    func testFoundationModelsConfigNamesTheInProcessBackend() throws {
        let repo = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let config = try Config.load(
            from: repo.appendingPathComponent("configs/foundation-models.yaml").path
        )
        XCTAssertEqual(config.backend, "foundation-models")
        // No endpoint or credential: the model is Apple's and there is no
        // process to forward environment to.
        XCTAssertNil(config.llm.baseURL)
        XCTAssertNil(config.llm.apiKey)
    }
}
