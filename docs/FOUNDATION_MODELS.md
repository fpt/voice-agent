# Apple Foundation Models as a second backend

How voice-agent gets an on-device agent loop, and the boundary that lets the
Swift frontend drive **either** a spawned app-server **or** Foundation Models
in-process.

Written 2026-08-03 against macOS 26.5 (arm64). Foundation Models moves fast —
re-verify the API surface before relying on details here.

## Verified

The framework is live on this machine:

```swift
SystemLanguageModel.default.availability  // → .available
```

A probe compiled with the installed toolchain imports `FoundationModels` and
reports the on-device model ready. It requires an M1 or later Mac with Apple
Intelligence **enabled** — a stricter bar than voice-agent's existing macOS 26
requirement, and one the Windows frontend cannot meet at all.

## Why in-process, and not a third app-server binary

The obvious move is a `voice-agent-fm app-server` binary alongside `gallium`
and `codex`. It would need no core changes at all — PR #4 made the backend
config-selectable, so `backend: "voice-agent-fm"` already works.

We are **not** doing that. Foundation Models is a Swift framework that runs
in-process; wrapping it in a subprocess would mean serialising JSON-RPC to
ourselves, and — more importantly — it would keep the screen-capture bridge
described below, whose entire reason to exist is that *Rust* cannot call macOS
APIs. In-process, that round trip disappears.

So the second backend lives in Swift, behind a boundary in `AgentKit`.

## The constraint that shapes the design

**4096 tokens, shared between input and output. Overflow throws — there is no
trimming strategy** ([TN3193](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)).
Private Cloud Compute raises the ceiling to 32K, still private, but off-device.

Measured against what we send today:

| | chars | ≈ tokens |
|---|---|---|
| `configs/system-prompt.md` | 600 | 150 |
| `skills/SKILL.md` | 2,745 | 686 |
| `skills/screen-analysis/SKILL.md` | 1,226 | 306 |
| **at rest, before any conversation** | **4,571** | **≈1,142 — 28% of budget** |

And it scales badly. `skill.rs` inlines each skill's *full* prompt on purpose:

> Each skill's full prompt is inlined (there is no lookup tool over the wire —
> the backend gets everything up front).

Four more skills and half the window is gone before the user speaks. Then one
`apply_ocr` over a screenshot can consume the rest — and note that PR #7
removed the 8000-char tool-output cap, because the only caller (`ToolRegistry`)
was itself dead. **The FM path needs that cap back, and tighter.**

This is why the plan below front-loads a measurement stage. If real turns do
not fit, the interesting work is context discipline, not the integration.

## The boundary

Today `AgentSession` is a thin wrapper that publishes `public let agent: Agent`,
and the CLI reaches straight through it — 13 call sites on the concrete UniFFI
type (`submitCaptureResult`, `setGoal`, `evaluateGoal`, `drainCaptureRequests`,
…). Nothing else can be substituted for it. That is the thing to fix first.

```
                 ┌─────────────────────────────┐
  VoiceAgentCli  │  AgentSession               │
  (REPL, TTS,    │    backend: AgentBackend ───┼──┬── AppServerBackend
   ambient loop) │    tts, config, skills      │  │     └─ UniFFI Agent ─→ gallium / codex
                 └─────────────────────────────┘  │
                                                  └── FoundationModelsBackend   (stage 2)
                                                        └─ LanguageModelSession
```

### What belongs in the protocol

Turn execution, conversation state, skills, goals, and ambient situation — the
things a frontend legitimately asks of *any* agent.

### What must not

`drainCaptureRequests` / `submitCaptureResult` are **not** agent operations.
They exist because Rust client tools cannot reach `WindowManager`, `OCR`, or
`ScreenCapture`, so Rust posts a request and a 100 ms poller in `main.swift`
answers it. Under Foundation Models, a `Tool` conformer calls those APIs
directly — no bridge, no poller, no 100 ms latency floor.

Baking the bridge into the protocol would therefore enshrine an app-server
implementation detail in the abstraction. Instead the protocol exposes it as an
**optional capability**:

```swift
var screenBridge: ScreenCaptureBridging? { get }   // app-server: self; FM: nil
```

The CLI starts its poller only when a backend offers one. That is the
difference between a boundary and a mechanical mirror of the UDL.

## Division of labour

| Concern | AppServerBackend | FoundationModelsBackend |
|---|---|---|
| Turn loop | `turn/start` → backend's ReAct | `LanguageModelSession` runs the tool loop itself |
| Tools | Rust client tools + capture bridge | Swift `Tool` conformers calling `ScreenCapture` directly |
| History | Rust `ConversationMemory` mirror | FM `Transcript` |
| Skills | inlined into developer instructions | Dynamic Profiles / Skill API |
| Goal eval | throwaway backend thread + lenient parse | `@Generable` verdict, structurally guaranteed |
| Approvals | `MutationApprover` over JSON-RPC | n/a until FM tools mutate anything |

Goal evaluation deserves a note. Today it opens a whole throwaway thread and
runs a full turn to answer yes/no, then parses the reply leniently —
`strip_think()`, try to extract JSON, fall back to a leading YES/NO, default to
"not met" when ambiguous. That machinery exists *because the output is not
structurally guaranteed*. `@Generable struct GoalVerdict { let met: Bool; let
reason: String }` deletes it by construction.

## Stages

### Stage 1 — the boundary (no behaviour change)

- `AgentBackend` protocol in `AgentKit`.
- `AppServerBackend` conforming to it, wrapping the UniFFI `Agent` and owning
  the capture bridge as an optional capability.
- `AgentSession` holds an `AgentBackend`; `public let agent: Agent` goes away.
- CLI call sites move off the concrete type.

Verifiable by the existing end-to-end checks: both backends must still complete
a real turn.

### Stage 2 — `FoundationModelsBackend` ✅

Done. `backend: "foundation-models"` (see `configs/foundation-models.yaml`)
runs the on-device model in-process — **no backend process is spawned at all**.
Availability is checked at startup and an unsupported machine falls back to the
app-server path rather than failing.

Three Swift `Tool` conformers (`list_windows`, `find_window`, `read_window`)
call `ScreenTools` directly. No capture bridge, no 100 ms poller.

**Measured context, which was the point of the stage:**

| turn | ≈tokens | % of 4096 | entries |
|---|---|---|---|
| plain reply | 701 | 17% | 1 |
| one tool call | 718 | 17% | 5 |
| two tool calls | 799 | 19% | 7 |

Comfortable, and the headroom comes from two decisions:

- **Skills are catalogued, not inlined.** The app-server path inlines each
  skill's full prompt (~992 tokens for two); a name-and-description line each
  costs a fraction of that. `addSkill` deliberately drops the prompt body.
- **Tool results are capped** at 1500 characters (`ScreenTools.maxResultChars`),
  roughly 375 tokens. `read_window` is OCR and would otherwise return tens of
  thousands of characters in a single call.

Turn latency is ~2 s, against gallium's ~90 s model load plus ~15 s per turn.

Two things worth knowing about the implementation:

- A session's instructions are **fixed at construction**, so changing them means
  a new session — and a new session has no transcript. That ruled out carrying
  ambient situation in the instructions: folding it in there silently wiped the
  conversation every 30 seconds, which is how often the frontend pushes a window
  list.
- **Ambient situation is a tool, never a prefix.** Moving it into the *turn
  input* fixed the wipe but broke something worse: prefixing every turn with
  "Recent screen activity: …" made the model read the whole turn as a query
  *about the screen*. "hi" came back as "I couldn't find any window displaying
  the text 'hi'". A small model does not reliably separate framing from the
  user's words. It is now `read_situation_messages`, matching the app-server
  backend — something the model reaches for, never something wrapped around what
  the user said.
- The backend is `@MainActor`-isolated rather than `@unchecked Sendable`: it
  holds mutable state touched by both the poller and turn execution, and its
  tools are MainActor-bound regardless.

### Stage 3 — context discipline

Stage 2 shipped the two cheap wins already (skill catalog instead of inlining,
tool-output cap). What is left, in the order the measurements suggest:

- **A skill lookup tool.** Today `addSkill` drops the prompt body entirely, so a
  skill is a name and a description and nothing more. The model should be able
  to fetch the body on demand.
- **Transcript trimming** via Dynamic Profiles, for long conversations. Today
  the only bound is `/reset`.
- **Private Cloud Compute escalation** (32K) for a turn that will not fit.
- **A real token count.** Everything above is `chars / 4`; Apple exposes no
  tokenizer. Newer SDKs are reported to add `tokenCount(for:)` — worth adopting
  when available, since overflow throws rather than truncates.

## Ambient situation, on both backends

The two paths now behave identically, which took bounding the producer as well as
the store:

- The window-list poller only pushes when the list **changed**. It used to push
  unconditionally every 30 s against a 10-minute window — twenty near-identical
  copies of the same titles, all of which the agent read back. Desktops are
  mostly static, so deduping at the source is what actually keeps this small.
- Both stores cap retained messages at **20** (`DEFAULT_MAX_MESSAGES` /
  `SituationStore.defaultMaxMessages`) on top of the 600 s TTL. Time alone bounds
  nothing against a fast producer; the cap means a chatty one degrades the
  context budget instead of destroying it.
- Both expose the same `read_situation_messages` tool. The Foundation Models
  version takes no arguments — the app-server one's session filtering and
  pagination are Claude-Code-specific and not worth the schema against a 4096
  token window.

## Errors, and what they revealed

Framework failures used to reach the user as raw `NSError` dumps
("Error Domain=FoundationModels.LanguageModelSession.GenerationError Code=-1 …"),
which say nothing. Turn failures are now mapped to a sentence, including the
underlying error — `localizedDescription` on a bridged framework error is
usually just "The operation couldn't be completed", with the cause buried in
`NSMultipleUnderlyingErrorsKey`.

Making them readable immediately corrected a wrong diagnosis. The first such
failure was recorded here as "transient token generation"; with the underlying
error surfaced it turned out to be `ReadWindowTool` failing with *"Failed to
start stream due to audio/video capture failure"* — a screen-recording
permission problem that **killed the entire turn**, because a throwing
`Tool.call` aborts the response.

The app-server backend does not behave that way: `item/tool/call` answers
`success: false` with the detail, which its own comment calls "a normal ReAct
outcome, not a transport error". The Foundation Models tools now match, so a
failing tool reports to the model and the turn continues.

The *detail* is the point, not just the failure. A screen-recording denial and a
transient glitch both localize to "The operation couldn't be completed", so the
report to the model digs the cause out of `NSMultipleUnderlyingErrorsKey` — the
model can then stop retrying a permission problem while retrying a flaky one.
Cancellation is deliberately rethrown rather than reported: it is not a tool
outcome, and the model should not reason about a turn being torn down.

### A real limit worth knowing

With that noise removed, one genuine failure remains. Measured on this machine:

| turn shape | outcome |
|---|---|
| plain chat | 4/4 succeeded |
| single tool call | 4/4 succeeded |
| open-ended, multi-tool chain | ~3/5 succeeded |

The failures are `com.apple.tokengeneration 10`, from the framework rather than
from our code, and they correlate with the length of the tool chain — each tool
result is charged to the same 4096 tokens the response has to fit in. `/reset`
and retry both clear it.

So: short exchanges and single tool calls are reliable; long tool chains are not
yet. Stage 3's context work (a skill lookup tool, transcript trimming, PCC
escalation) is aimed squarely at this.

## Risks

- **Device gating.** M1+ with Apple Intelligence enabled. Needs a real
  `availability` check and a graceful fallback, not a crash.
- **Guardrails.** FM can refuse content; a refusal must not look like a turn
  failure.
- **No public tokenizer.** Token estimation is heuristic, which is
  uncomfortable when overflow is a hard error rather than a truncation.
- **Windows.** The C# frontend gets none of this. The boundary must not
  regress it — it keeps the app-server path unchanged.
- **Divergence.** Two backends means two behaviours to keep honest. The
  protocol is the contract; anything that only works on one side belongs behind
  an optional capability, like the screen bridge.

## Sources

- [TN3193: Managing the on-device foundation model's context window](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)
- [WWDC26 — Build agentic app experiences with the Foundation Models framework](https://developer.apple.com/videos/play/wwdc2026/242/) (Dynamic Profiles, Skill API)
- [WWDC26 — Bring an LLM provider to the Foundation Models framework](https://developer.apple.com/videos/play/wwdc2026/339/) (`LanguageModel` / `LanguageModelExecutor`)
- [The Tool protocol](https://blakecrosley.com/blog/foundation-models-on-device-llm)
- [Making the most of the context window](https://zats.io/blog/making-the-most-of-apple-foundation-models-context-window/)
