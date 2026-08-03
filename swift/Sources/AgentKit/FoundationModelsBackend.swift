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
/// Its mutable state is reached from two directions — the frontend's window
/// poller (MainActor) pushes situation while a turn may be executing on any
/// executor — so every access goes through `lock`.
///
/// `@MainActor` isolation would be the nicer answer, but `AgentBackend` inherits
/// `Sendable` and Swift will not form an actor-isolated conformance to a
/// Sendable-inheriting protocol. So the synchronisation is explicit, and
/// `@unchecked` is backed by the lock rather than by assertion.
@available(macOS 26.0, *)
public final class FoundationModelsBackend: AgentBackend, @unchecked Sendable {

    /// Guards short reads and writes of the stored properties below. Never held
    /// across an `await`.
    private let lock = NSLock()

    /// Serialises everything that *uses* the session, across suspension points.
    ///
    /// `lock` cannot do this job. Snapshotting the session reference under it
    /// protects only the pointer: two turns would still call `respond` on the
    /// same session, and `reset` / `setSystemPrompt` could replace it while a
    /// response was still running.
    private let sessionGate = AsyncGate()

    /// Bumped by every `setGoal` / `clearGoal`. `evaluateGoal` captures it before
    /// awaiting and refuses to apply its verdict if it no longer matches — a
    /// judgement about the previous goal must not clear or annotate a new one.
    private var goalEpoch: UInt64 = 0

    private func locked<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    private let logger = Logger("FoundationModels")
    private let screenTools: ScreenTools

    /// Rebuilt whenever instructions change (a session's instructions are fixed
    /// at construction) and on `reset()`.
    private var session: LanguageModelSession
    private var systemPrompt: String = ""
    private var skills: [(name: String, description: String)] = []

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
    private func rebuildSessionLocked() {
        session = LanguageModelSession(
            tools: FoundationModelsBackend.tools(screenTools),
            instructions: buildInstructionsLocked()
        )
    }

    private func buildInstructionsLocked() -> String {
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
        await sessionGate.acquire()
        defer { sessionGate.release() }

        let session = locked { self.session }
        let reply = try await session.respond(to: text)
        logTranscript()
        return Self.response(reply.content, contextPercent: contextPercent())
    }

    /// Share of the window the transcript occupies, 0-100. Estimated at ~4
    /// chars/token — Apple exposes no tokenizer — but the trend is what matters,
    /// and overflow is an error rather than a truncation.
    private func contextPercent() -> Float {
        let session = locked { self.session }
        let chars = session.transcript.reduce(0) { $0 + String(describing: $1).count }
        return Float(chars / 4) / Float(Self.contextWindowTokens) * 100
    }

    private static let contextWindowTokens = 4096

    private func logTranscript() {
        let session = locked { self.session }
        let chars = session.transcript.reduce(0) { $0 + String(describing: $1).count }
        let approxTokens = chars / 4
        let pct = Int(Double(approxTokens) / Double(Self.contextWindowTokens) * 100)
        logger.info("transcript ≈\(approxTokens) tokens (~\(pct)% of 4096), \(session.transcript.count) entries")
        if pct > 70 {
            logger.warning("context is filling up; a turn that overflows will fail rather than truncate")
        }
    }

    public func reset() async {
        // A fresh session is the only way to clear a transcript, which is also
        // the only bound on context growth we have. Taking the gate means it can
        // never replace a session a turn is still responding on.
        await sessionGate.acquire()
        defer { sessionGate.release() }
        locked { rebuildSessionLocked() }
    }

    public func conversationHistory() async -> String {
        await sessionGate.acquire()
        defer { sessionGate.release() }
        return locked { self.session }.transcript
            .map { String(describing: $0) }
            .joined(separator: "\n")
    }

    // MARK: Configuration

    public func setSystemPrompt(_ prompt: String) async {
        await sessionGate.acquire()
        defer { sessionGate.release() }
        locked {
            systemPrompt = prompt
            rebuildSessionLocked()
        }
    }

    public func addSkill(name: String, description: String, prompt: String) async {
        await sessionGate.acquire()
        defer { sessionGate.release() }
        // `prompt` is intentionally dropped: inlining full skill prompts does not
        // fit this model's window. Stage 3 gives the model a lookup tool so it
        // can fetch one on demand.
        locked {
            skills.append((name: name, description: description))
            rebuildSessionLocked()
        }
    }

    // MARK: Ambient context

    /// Deliberately ignored on this path.
    ///
    /// The app-server backend needs ambient pushes because its Rust tools cannot
    /// see the screen between turns. Here `list_windows` reads the *live* window
    /// list on demand, so a buffered copy is both redundant and expensive
    /// against a 4096-token window.
    ///
    /// It was worse than expensive: prepending the buffer to each turn as
    /// "Recent screen activity: …\n\n<user text>" made a small model read the
    /// whole thing as one query *about the screen*. Every message — "hi", "how
    /// are you?" — came back as "I couldn't find any window displaying …".
    /// Ambient context has to be something the model reaches for, not something
    /// wrapped around what the user said.
    public func pushSituationMessage(text: String, source: String, sessionId: String) {}

    // MARK: Goals

    public func setGoal(condition: String) {
        locked {
            goalEpoch &+= 1
            goalCondition = condition
            goalStartedAt = Date()
            goalTurns = 0
            goalLastReason = nil
        }
    }

    public func clearGoal() {
        locked {
            goalEpoch &+= 1
            goalCondition = nil
            goalStartedAt = nil
            goalTurns = 0
            goalLastReason = nil
        }
    }

    public func goalStatus() -> GoalStatus? {
        let (condition, started, turns, reason) = locked {
            (goalCondition, goalStartedAt, goalTurns, goalLastReason)
        }
        guard let condition, let started else { return nil }
        return GoalStatus(
            condition: condition,
            elapsedSeconds: UInt64(Date().timeIntervalSince(started)),
            turnsEvaluated: turns,
            lastReason: reason
        )
    }

    public func evaluateGoal() async throws -> GoalEvaluation {
        let snapshot = locked { () -> (condition: String, epoch: UInt64)? in
            guard let c = goalCondition else { return nil }
            goalTurns += 1
            return (c, goalEpoch)
        }
        guard let snapshot else {
            return GoalEvaluation(met: false, reason: "No active goal.")
        }
        let condition = snapshot.condition

        // A separate, tool-less session: evaluation must not see the screen or
        // add to the working transcript.
        let judge = LanguageModelSession(
            instructions: """
            You judge whether a goal has been met, given a transcript. \
            Be strict: say met only when the transcript clearly shows it.
            """
        )
        let transcript = await Self.cap(conversationHistory(), 2000)
        let verdict = try await judge.respond(
            to: "Goal: \(condition)\n\nTranscript:\n\(transcript)\n\nHas the goal been met?",
            generating: GoalVerdict.self
        )
        // Apply only if this verdict still describes the current goal. `setGoal`
        // and `clearGoal` are callable throughout the await above, so a slow
        // evaluation could otherwise annotate — or, worse, clear — a goal it
        // never judged.
        //
        // Clearing on success is part of the contract, not an optimisation:
        // `GoalDriver` breaks its loop on `met` *without* clearing, on the
        // stated assumption that the backend already did (the Rust path does
        // `*g = None` there). Leaving it set makes `/goal` keep reporting an
        // achieved goal as active.
        let applied = locked { () -> Bool in
            guard goalEpoch == snapshot.epoch else { return false }
            goalLastReason = verdict.content.reason
            if verdict.content.met {
                goalEpoch &+= 1
                goalCondition = nil
                goalStartedAt = nil
                goalTurns = 0
                goalLastReason = nil
            }
            return true
        }
        if !applied {
            logger.info("goal changed during evaluation; discarding a verdict for the previous goal")
        }

        return GoalEvaluation(met: verdict.content.met, reason: verdict.content.reason)
    }

    // MARK: Optional capabilities

    /// Tools here are Swift and call the screen APIs directly, so there is
    /// nothing for the frontend to service.
    public var screenBridge: ScreenCaptureBridging? { nil }

    public nonisolated var backendDescription: BackendDescription {
        BackendDescription(
            model: "Apple on-device model",
            endpoint: "on-device (no backend process)"
        )
    }

    // MARK: Helpers

    private static func cap(_ s: String, _ limit: Int) -> String {
        s.count <= limit ? s : String(s.suffix(limit))
    }

    /// `AgentResponse` is a UniFFI record shared with the app-server path. Token
    /// counts are zero: Apple exposes no tokenizer, and reporting a guess as a
    /// measurement would be worse than reporting nothing.
    private static func response(_ content: String, contextPercent: Float = 0) -> AgentResponse {
        AgentResponse(
            content: content,
            role: "assistant",
            isFinal: true,
            keywords: nil,
            reasoning: nil,
            inputTokens: 0,
            outputTokens: 0,
            totalTokens: 0,
            contextPercent: contextPercent,
            suggestedNextCheckSeconds: nil
        )
    }
}
#endif
