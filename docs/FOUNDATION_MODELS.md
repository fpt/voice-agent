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
  a new session — and a new session has no transcript. Ambient situation
  therefore rides along with the *turn input*, never the instructions. Folding
  it into instructions silently wiped the conversation every 30 seconds, which
  is how often the frontend pushes a window list.
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
