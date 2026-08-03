import AgentBridge
import Foundation

/// ``AgentBackend`` backed by a spawned app-server (`gallium`, `codex`, or any
/// binary speaking the same codex-app-server JSON-RPC subset).
///
/// Thin by design: the Rust core already owns the client, the conversation
/// mirror, goals, and situation state. This type exists so the frontend depends
/// on the ``AgentBackend`` protocol rather than on the concrete UniFFI `Agent`,
/// which is what makes a second backend substitutable.
public final class AppServerBackend: AgentBackend, ScreenCaptureBridging, @unchecked Sendable {

    /// The UniFFI handle. Internal on purpose — reaching past the protocol is
    /// exactly what this class exists to prevent.
    private let agent: Agent

    public init(agent: Agent) {
        self.agent = agent
    }

    // MARK: Turns

    /// `agent.step` blocks for the whole turn (a UniFFI call into Rust, which
    /// waits on `turn/completed`). Run it on a detached task so it never occupies
    /// a cooperative thread.
    public func step(_ text: String) async throws -> AgentResponse {
        let agent = self.agent
        return try await Task.detached { try agent.step(userInput: text) }.value
    }

    public func reset() {
        agent.reset()
    }

    public func conversationHistory() -> String {
        agent.getConversationHistory()
    }

    // MARK: Configuration

    public func setSystemPrompt(_ prompt: String) {
        agent.setSystemPrompt(prompt: prompt)
    }

    public func addSkill(name: String, description: String, prompt: String) {
        agent.addSkill(name: name, description: description, prompt: prompt)
    }

    // MARK: Ambient context

    public func pushSituationMessage(text: String, source: String, sessionId: String) {
        agent.pushSituationMessage(text: text, source: source, sessionId: sessionId)
    }

    // MARK: Goals

    public func setGoal(condition: String) {
        agent.setGoal(condition: condition)
    }

    public func clearGoal() {
        agent.clearGoal()
    }

    public func goalStatus() -> GoalStatus? {
        agent.goalStatus()
    }

    public func evaluateGoal() async throws -> GoalEvaluation {
        let agent = self.agent
        return try await Task.detached { try agent.evaluateGoal() }.value
    }

    // MARK: Optional capabilities

    /// This backend's tools run in Rust, which cannot call the macOS screen
    /// APIs, so it does need the frontend to service capture requests.
    public var screenBridge: ScreenCaptureBridging? { self }

    public func drainCaptureRequests() -> [CaptureRequest] {
        agent.drainCaptureRequests()
    }

    public func submitCaptureResult(id: String, imageBase64: String, metadataJson: String) {
        agent.submitCaptureResult(id: id, imageBase64: imageBase64, metadataJson: metadataJson)
    }
}
