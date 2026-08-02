# Mid-turn steering for voice

How voice-agent should let a user correct the agent **while it is working**, and
what has to be fixed first.

Written 2026-08-02 against `../codex` @ `18f50c9e62` and `../rs-gallium` @ the
tree checked out beside this repo. Both are moving targets — re-verify the
protocol shapes before relying on them.

## Why this matters for voice specifically

Voice has a property text does not: the user is *listening while the agent
works*, and speaking costs nothing. When you hear the agent narrate the wrong
move, the natural reaction is to say "no, not that file."

In a text REPL that correction has somewhere to go — you read, you interrupt,
you retype. In voice, today, the utterance is either dropped (the mic is gated)
or queued into the *next* turn, where it arrives after the damage is done.

`turn/steer` puts the correction into the turn the model is already executing.
That is the single highest-value protocol feature for this app.

## Blocker: the turn loop does not work today

`AppServerClient::run_turn_on` assumed a backend that runs the turn *inside* the
`turn/start` request and answers when it is finished. Neither backend does that
any more.

Probed against a live `gallium app-server`:

```
-> {"id":3,"method":"turn/start","params":{"threadId":"thread_1","input":[…]}}
<- {"id":3,"result":{"turn":{"id":"turn_1","status":"inProgress"}}}          10:32:45
<- {"method":"item/completed","params":{…,"turnId":"turn_1",
                                        "item":{"type":"agentMessage","text":"Hi! …"}}}   10:34:31
<- {"method":"turn/completed","params":{"threadId":"thread_1",
                                        "turn":{"id":"turn_1","status":"completed"}}}
```

Two false assumptions:

1. **The turn id is not at `turnId`.** Both backends answer with
   `{"turn":{"id":…}}` — codex's `TurnStartResponse = { turn: Turn }`
   (`app-server-protocol/schema/typescript/v2/TurnStartResponse.ts`), gallium's
   literal `json!({"turn":{"id":turn_id,"status":"inProgress"}})`
   (`appserver/server.rs:600`). Reading `resp["turnId"]` yields `""`.
2. **The response does not mean "done".** Both answer immediately and run the
   turn in the background. gallium says so outright at `server.rs:570`:
   *"Answering `turn/start` immediately is what codex does, and what makes a turn
   interruptible at all: a reply that only arrives once the turn is over cannot
   be the thing a client waits on while stopping it."*

So `step()` returned an empty string almost instantly — 106 seconds before the
model had even answered, in the trace above.

The unit tests passed because the stub returned `{"turnId":"turn1"}` and emitted
`item/completed` *before* answering: the old synchronous gallium contract, which
no shipping backend honours. A mock that drifted from reality and then certified
the drift.

**Stage 0 below fixes this, and is worth doing on its own merits.** It also
happens to be the prerequisite for steering, because steering needs the live
turn id that the fix has to start tracking anyway.

## What the backends offer

| Method | Shape | codex | gallium |
|---|---|---|---|
| `turn/start` | → `{turn:{id,status}}`, async | ✅ | ✅ |
| `turn/interrupt` | `{threadId,turnId}` → `{}`, answers once stopped | ✅ | ✅ |
| `turn/steer` | `{threadId,input[],expectedTurnId,clientUserMessageId?}` → `{turnId}` | ✅ | ❌ |
| `thread/realtime/*` | 8 notifications + `thread/realtime/start` | experimental | ❌ |

Completion notifications:

| Notification | codex | gallium |
|---|---|---|
| `turn/completed` `{threadId,turn:{id,status}}` | ✅ | ✅ |
| `turn/failed` `{threadId,turnId,error}` | ✗ (folds into `turn/completed`) | ✅ |
| `item/completed` `{threadId,turnId,item}` | ✅ | ✅ |

gallium's `turn/failed` is a deliberate divergence, documented at
`server.rs:761-767`: codex spells every ending as `turn/completed`, but clients
key off the method rather than the status, so folding failures in would turn
every failure into a silent success. A client must handle both.

### `turn/steer` semantics

Injects user input into the in-flight turn; the model picks it up as pending
input at its next loop boundary. No interrupt, no restart, no lost work.

`expectedTurnId` is a required precondition — the request fails if it is not the
currently active turn, which is what makes it safe to fire from a racing thread.
Errors (`app-server/src/request_processors/turn_processor.rs:920`):

- `NoActiveTurn` — the turn ended between our decision and our request
- `ExpectedTurnMismatch` — a different turn is now active
- `ActiveTurnNotSteerable { turn_kind: review | compact }`
- `EmptyInput`

All four are **normal outcomes for a voice client**, not transport errors. They
should map to a typed result the frontend can react to, never to a failed turn.

### `thread/realtime/*` — noted, not proposed

Codex can bridge OpenAI's Realtime API (WebRTC/WS, streamed input/output audio
and live transcripts) through the app-server. It is `#[experimental]` and gated
behind `Feature::RealtimeConversation`.

It would replace voice-agent's entire STT/TTS stack with OpenAI's, which ends the
local-model story and makes the app codex-only. Out of scope here; recorded so
the option is known.

## Feasibility: the transport already supports steering

The thing that could have killed this design does not.

`Connection::request` (`appserver/rpc.rs:108-132`) allocates a unique id,
registers a per-request oneshot in `pending`, writes, then blocks on the
receiver. `write_msg` holds the writer lock only across the write, never across
the wait. Inbound responses are routed by id from the reader thread.

So a second thread can send `turn/steer` while `turn/start` is still outstanding.
**No transport change is required.**

## Staged plan

### Stage 0 — make the turn loop event-driven *(prerequisite; fixes a live bug)*

- Read the turn id from `turn.id`, falling back to `turnId` for any backend still
  on the old shape.
- Wait for `turn/completed` / `turn/failed` for that id instead of treating the
  `turn/start` response as completion.
- Accumulate `agentMessage` items per turn rather than overwriting: a steered
  turn can emit several, and today the last one silently wins.
- Handle notifications that arrive *before* the response — create the turn's slot
  on demand, since ordering is not guaranteed.
- Wake waiters when the connection closes, so a dead backend fails instead of
  hanging forever.
- Rewrite the stubs to the real async contract: answer first, notify after.

### Stage 1 — `steer` in the core

- Track `{thread_id, turn_id}` as the active turn on `AppServerClient`; set it
  when `turn/start` is answered, clear it on completion.
- `AppServerClient::steer(text)` sending `turn/steer` with `expectedTurnId`.
- Return a typed outcome — `Steered` / `NoActiveTurn` / `NotSteerable` /
  `Unsupported` — rather than an error.
- Probe once and cache `Unsupported` on method-not-found, falling back to
  queue-until-turn-ends. That keeps gallium working unchanged.
- `Agent::steer(text)` + the UDL entry; append to the conversation mirror.

### Stage 2 — mid-turn correction in the voice REPL

- Keep STT live during the agent's working phase. The mic is gated only for TTS
  half-duplex, and there is no TTS while the agent works — that window is exactly
  when steering is useful.
- A final transcript arriving while a turn is active routes to `agent.steer()`
  instead of being queued.
- **Do not take `turnGate`.** Steering is not a turn; taking the gate would
  deadlock against the turn being steered.
- Speak a two-word acknowledgement. Voice has no visual feedback, so a
  correction that lands silently is indistinguishable from one that was dropped.

### Stage 3 — barge-in

- "stop" / "wait" / "no" during a turn → `turn/interrupt` rather than steer.
  `RuleBasedBackchannelDetector` (`state_updater.rs`) already classifies short
  utterances and could carry a stop-word class.
- This is the stage that forces revisiting half-duplex: barging in during TTS
  means keeping the mic open during playback, which needs the echo cancellation
  in [VOICE_PROCESSING_IO.md](VOICE_PROCESSING_IO.md).
- Benefits both backends — gallium implements `turn/interrupt`.

## Caveat

gallium does not implement `turn/steer`, so Stages 1-2 are codex-only until it
does. Stages 0 and 3 benefit both backends.
