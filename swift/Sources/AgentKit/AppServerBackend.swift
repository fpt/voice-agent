import AgentBridge
import AgentCore
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

    /// The program Rust will have spawned, for the banner only.
    private let program: String
    private let model: String?

    public init(agent: Agent, program: String, model: String?) {
        self.agent = agent
        self.program = program
        self.model = model
    }

    public var backendDescription: BackendDescription {
        BackendDescription(
            model: model ?? "(backend default)",
            endpoint: "\(program) app-server"
        )
    }

    // MARK: Turns

    /// `agent.step` blocks for the whole turn (a UniFFI call into Rust, which
    /// waits on `turn/completed`). Run it on a detached task so it never occupies
    /// a cooperative thread.
    public func step(_ text: String) async throws -> AgentCore.AgentResponse {
        let agent = self.agent
        let generated = try await Task.detached { try agent.step(userInput: text) }.value
        return Self.mapped(generated)
    }

    public func reset() async {
        agent.reset()
    }

    public func conversationHistory() async -> String {
        agent.getConversationHistory()
    }

    // MARK: Configuration

    public func setSystemPrompt(_ prompt: String) async {
        agent.setSystemPrompt(prompt: prompt)
    }

    public func addSkill(name: String, description: String, prompt: String) async {
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

    public func goalStatus() -> AgentCore.GoalStatus? {
        agent.goalStatus().map {
            AgentCore.GoalStatus(
                condition: $0.condition,
                elapsedSeconds: $0.elapsedSeconds,
                turnsEvaluated: $0.turnsEvaluated,
                lastReason: $0.lastReason
            )
        }
    }

    public func evaluateGoal() async throws -> AgentCore.GoalEvaluation {
        let agent = self.agent
        let verdict = try await Task.detached { try agent.evaluateGoal() }.value
        return AgentCore.GoalEvaluation(met: verdict.met, reason: verdict.reason)
    }

    // MARK: Optional capabilities

    /// This backend's tools run in Rust, which cannot call the macOS screen
    /// APIs, so it does need the frontend to service capture requests.
    public var screenBridge: ScreenCaptureBridging? { self }

    /// Translate the UniFFI record into the domain type.
    ///
    /// This mapping is the whole point of the split: it is the only place the
    /// generated types cross into the rest of the app, so everything above can
    /// build without the Rust cdylib.
    private static func mapped(_ r: AgentBridge.AgentResponse) -> AgentCore.AgentResponse {
        AgentCore.AgentResponse(
            content: r.content,
            role: r.role,
            isFinal: r.isFinal,
            keywords: r.keywords,
            reasoning: r.reasoning,
            inputTokens: r.inputTokens,
            outputTokens: r.outputTokens,
            totalTokens: r.totalTokens,
            contextPercent: r.contextPercent,
            suggestedNextCheckSeconds: r.suggestedNextCheckSeconds
        )
    }

    public func drainCaptureRequests() -> [AgentCore.CaptureRequest] {
        agent.drainCaptureRequests().map {
            AgentCore.CaptureRequest(
                id: $0.id,
                windowId: $0.windowId,
                cropX: $0.cropX,
                cropY: $0.cropY,
                cropW: $0.cropW,
                cropH: $0.cropH,
                detect: $0.detect,
                applyOcr: $0.applyOcr,
                searchKeywords: $0.searchKeywords
            )
        }
    }

    public func submitCaptureResult(id: String, imageBase64: String, metadataJson: String) {
        agent.submitCaptureResult(id: id, imageBase64: imageBase64, metadataJson: metadataJson)
    }
}
