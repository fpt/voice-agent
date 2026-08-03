import AgentBridge
import Foundation
import Util

#if canImport(FoundationModels)
import FoundationModels

// MARK: - Tools

/// Screen tools exposed to the model. Each calls ``ScreenTools`` directly —
/// there is no capture bridge and no poller on this path.

@available(macOS 26.0, *)
struct ListWindowsTool: Tool {
    let name = "list_windows"
    let description =
        "List the user's currently open windows (app name and title). Use this first to see what they are working on."

    @Generable
    struct Arguments {}

    let tools: ScreenTools

    func call(arguments: Arguments) async throws -> String {
        try await tools.listWindows()
    }
}

@available(macOS 26.0, *)
struct FindWindowTool: Tool {
    let name = "find_window"
    let description = "Find open windows whose app name or title matches some keywords."

    @Generable
    struct Arguments {
        @Guide(description: "Words to match against window titles and app names")
        var keywords: String
    }

    let tools: ScreenTools

    func call(arguments: Arguments) async throws -> String {
        try await tools.findWindow(keywords: arguments.keywords)
    }
}

@available(macOS 26.0, *)
struct ReadWindowTool: Tool {
    let name = "read_window"
    let description =
        "Read the visible text of a window by its title or app name, via OCR. Results are truncated, so prefer a specific window."

    @Generable
    struct Arguments {
        @Guide(description: "Title or app name of the window to read")
        var window: String
    }

    let tools: ScreenTools

    func call(arguments: Arguments) async throws -> String {
        try await tools.readWindow(titleOrProcess: arguments.window)
    }
}

/// Verdict shape for goal evaluation.
///
/// The whole point of `@Generable` here: the app-server path asks a model for a
/// yes/no in prose and then parses it leniently — strip `<think>`, try to
/// extract JSON, fall back to a leading YES/NO, default to "not met" when
/// ambiguous. None of that is needed when the type is guaranteed.
@available(macOS 26.0, *)
@Generable
struct GoalVerdict {
    @Guide(description: "true only if the condition is clearly satisfied")
    var met: Bool
    @Guide(description: "one short sentence justifying the decision")
    var reason: String
}

// MARK: - Backend

/// ``AgentBackend`` running Apple's on-device model in-process.
///
/// Requires macOS 26 on Apple silicon with Apple Intelligence enabled. Build it
/// with ``make(...)``, which returns `nil` when the device cannot run it so the
/// caller can fall back to the app-server path.
///
/// The governing constraint is a **4096-token window shared between input and
/// output, with a hard error on overflow** — see docs/FOUNDATION_MODELS.md.
/// Everything here is shaped by that: tool results are capped, skills are
/// summarised rather than inlined, and every turn logs its transcript size.
/// `@MainActor`-isolated: it holds mutable session/skill/situation state touched
/// by both the frontend's window poller and turn execution, and its tools call
/// `ScreenTools`, which is MainActor-bound anyway. Isolation rather than
/// `@unchecked Sendable` means the compiler proves the absence of the data race
/// instead of us asserting it.
@available(macOS 26.0, *)
@MainActor
public final class FoundationModelsBackend: AgentBackend {

    private let logger = Logger("FoundationModels")
    private let screenTools: ScreenTools

    /// Rebuilt whenever instructions change (a session's instructions are fixed
    /// at construction) and on `reset()`.
    private var session: LanguageModelSession
    private var systemPrompt: String = ""
    private var skills: [(name: String, description: String)] = []

    /// Ambient observations, newest last. Bounded hard: this is context the model
    /// pays for on every turn.
    private var situation: [String] = []
    private static let maxSituationEntries = 3

    private var goalCondition: String?
    private var goalStartedAt: Date?
    private var goalTurns: UInt32 = 0
    private var goalLastReason: String?

    /// `nil` when the device cannot run the on-device model — no Apple
    /// Intelligence, unsupported hardware, or the model still downloading.
    public static func make(screenTools: ScreenTools) -> FoundationModelsBackend? {
        let model = SystemLanguageModel.default
        switch model.availability {
        case .available:
            return FoundationModelsBackend(screenTools: screenTools)
        case .unavailable(let reason):
            Logger("FoundationModels").warning(
                "on-device model unavailable (\(reason)); falling back to the app-server backend"
            )
            return nil
        @unknown default:
            Logger("FoundationModels").warning(
                "on-device model availability unknown; falling back to the app-server backend"
            )
            return nil
        }
    }

    private init(screenTools: ScreenTools) {
        self.screenTools = screenTools
        self.session = LanguageModelSession(
            tools: FoundationModelsBackend.tools(screenTools),
            instructions: ""
        )
        logger.info("on-device model ready")
    }

    private static func tools(_ t: ScreenTools) -> [any Tool] {
        [ListWindowsTool(tools: t), FindWindowTool(tools: t), ReadWindowTool(tools: t)]
    }

    // MARK: Instructions

    /// A session's instructions are fixed at construction, so changing the system
    /// prompt or skill catalog means a new session — and a new session has no
    /// transcript. Both happen once, during init. Nothing on the hot path may
    /// call this: see `turnInput(for:)` for why ambient context does not.
    private func rebuildSession() {
        session = LanguageModelSession(
            tools: FoundationModelsBackend.tools(screenTools),
            instructions: buildInstructions()
        )
    }

    private func buildInstructions() -> String {
        var parts: [String] = []
        if !systemPrompt.isEmpty { parts.append(systemPrompt) }

        // Skills are listed by name and description only. The app-server path
        // inlines each skill's full prompt ("the backend gets everything up
        // front"), which measured ~992 tokens for two skills — a quarter of this
        // model's entire window. A catalog costs a line each.
        if !skills.isEmpty {
            let lines = skills.map { "- \($0.name): \($0.description)" }.joined(separator: "\n")
            parts.append("Available skills:\n\(lines)")
        }

        return parts.joined(separator: "\n\n")
    }

    // MARK: Turns

    public func step(_ text: String) async throws -> AgentResponse {
        let reply = try await session.respond(to: turnInput(for: text))
        logTranscript()
        return Self.response(reply.content)
    }

    /// Ambient observations ride along with the *turn*, never the instructions.
    ///
    /// A session's instructions are fixed at construction, so folding situation
    /// into them meant rebuilding the session on every push — and the frontend
    /// pushes a window list every 30 seconds, which silently wiped the
    /// conversation mid-chat. Consumed once: it is context about *now*.
    private func turnInput(for text: String) -> String {
        guard !situation.isEmpty else { return text }
        let ambient = situation.joined(separator: "\n")
        situation.removeAll()
        return "Recent screen activity:\n\(ambient)\n\n\(text)"
    }

    /// Report how much of the window the transcript now occupies. Estimated —
    /// Apple exposes no tokenizer — but the trend is what matters, and overflow
    /// is an error rather than a truncation, so it is worth watching.
    private func logTranscript() {
        let chars = session.transcript.reduce(0) { total, entry in
            total + String(describing: entry).count
        }
        let approxTokens = chars / 4
        let pct = Int(Double(approxTokens) / 4096.0 * 100)
        logger.info("transcript ≈\(approxTokens) tokens (~\(pct)% of 4096), \(session.transcript.count) entries")
        if pct > 70 {
            logger.warning("context is filling up; a turn that overflows will fail rather than truncate")
        }
    }

    public func reset() {
        // A fresh session is the only way to clear a transcript, which is also
        // the only bound on context growth we have.
        rebuildSession()
        situation.removeAll()
    }

    public func conversationHistory() -> String {
        session.transcript
            .map { String(describing: $0) }
            .joined(separator: "\n")
    }

    // MARK: Configuration

    public func setSystemPrompt(_ prompt: String) {
        systemPrompt = prompt
        rebuildSession()
    }

    public func addSkill(name: String, description: String, prompt: String) {
        // `prompt` is intentionally dropped: inlining full skill prompts does not
        // fit this model's window. Stage 3 gives the model a lookup tool so it
        // can fetch one on demand.
        skills.append((name: name, description: description))
        rebuildSession()
    }

    // MARK: Ambient context

    public func pushSituationMessage(text: String, source: String, sessionId: String) {
        situation.append(text)
        if situation.count > Self.maxSituationEntries {
            situation.removeFirst(situation.count - Self.maxSituationEntries)
        }
    }

    // MARK: Goals

    public func setGoal(condition: String) {
        goalCondition = condition
        goalStartedAt = Date()
        goalTurns = 0
        goalLastReason = nil
    }

    public func clearGoal() {
        goalCondition = nil
        goalStartedAt = nil
        goalTurns = 0
        goalLastReason = nil
    }

    public func goalStatus() -> GoalStatus? {
        guard let condition = goalCondition, let started = goalStartedAt else { return nil }
        return GoalStatus(
            condition: condition,
            elapsedSeconds: UInt64(Date().timeIntervalSince(started)),
            turnsEvaluated: goalTurns,
            lastReason: goalLastReason
        )
    }

    public func evaluateGoal() async throws -> GoalEvaluation {
        guard let condition = goalCondition else {
            return GoalEvaluation(met: false, reason: "No active goal.")
        }
        goalTurns += 1

        // A separate, tool-less session: evaluation must not see the screen or
        // add to the working transcript.
        let judge = LanguageModelSession(
            instructions: """
            You judge whether a goal has been met, given a transcript. \
            Be strict: say met only when the transcript clearly shows it.
            """
        )
        let transcript = Self.cap(conversationHistory(), 2000)
        let verdict = try await judge.respond(
            to: "Goal: \(condition)\n\nTranscript:\n\(transcript)\n\nHas the goal been met?",
            generating: GoalVerdict.self
        )
        goalLastReason = verdict.content.reason
        return GoalEvaluation(met: verdict.content.met, reason: verdict.content.reason)
    }

    // MARK: Optional capabilities

    /// Tools here are Swift and call the screen APIs directly, so there is
    /// nothing for the frontend to service.
    public var screenBridge: ScreenCaptureBridging? { nil }

    // MARK: Helpers

    private static func cap(_ s: String, _ limit: Int) -> String {
        s.count <= limit ? s : String(s.suffix(limit))
    }

    /// `AgentResponse` is a UniFFI record shared with the app-server path. Token
    /// counts are zero: Apple exposes no tokenizer, and reporting a guess as a
    /// measurement would be worse than reporting nothing.
    private static func response(_ content: String) -> AgentResponse {
        AgentResponse(
            content: content,
            role: "assistant",
            isFinal: true,
            keywords: nil,
            reasoning: nil,
            inputTokens: 0,
            outputTokens: 0,
            totalTokens: 0,
            contextPercent: 0,
            suggestedNextCheckSeconds: nil
        )
    }
}
#endif
