# Task 3b — `locode-protocol`: streaming event protocol (`stream-json`) + reconstruction

**Retrospective plan.** This task is **already implemented and merged** (`tasks/todo.md`
Task 3b ✅; commit `1392552`). This document is the pre-implementation plan we skipped,
recording **what is actually built and why**, grounded in the shipped source and the studied
harnesses. It does not propose changes.

Source of truth: ADR-0014 (streaming event protocol), ADR-0009 (headless I/O contract),
ADR-0013 (conversation model), `SPEC.md`. Task 3b builds directly on Task 3 (see
`task-03-protocol-conversation-report.md`) — same crate, same file — and reuses its
`Message`/`Report`/`Conversation` types wholesale.

As-built code: `crates/locode-protocol/src/lib.rs:246–312` (the `Event` enum +
`reconstruct_conversation`), tests at `lib.rs:424–524`.

Submodule roots (abbreviated in citations):
- `grok` = `~/dev/coding-cli-survey/submodules/grok-build`
- `codex` = `~/dev/coding-cli-survey/submodules/codex`
- survey = `~/dev/coding-cli-survey/survey`
- note = the `claude-code-system-surfaces` memory (proxy capture + Claude Code source)

---

## 1. Purpose & scope

Define the **`stream-json` trajectory format**: a JSONL event stream (one JSON object per line,
`#[serde(tag="type")]`) that is a **self-sufficient, replayable source of a whole run** — plus
`reconstruct_conversation`, its inverse, which rebuilds the full `Conversation` (System/Developer
included) from the events **alone**, with no side channel.

The load-bearing idea (ADR-0014): the maintainer's `swe-lab` already reconstructs Claude Code
history from `claude -p … --output-format stream-json --verbose`, but **Claude's stream omits
`system` and `tools`**, forcing a *second* capture (a reverse proxy) to recover them. locode
closes that gap: the first event (`Init`) carries the base prompt + tool specs + model, so the
stream reconstructs with nothing else. `stream-json` is a **first-class v0 output mode**, not a
deferred seam (ADR-0014 reprioritizes over the "single-JSON first" lean in
`docs/design/report-envelope.md`).

Types only live here; the **loop emits** them (Task 6, via an `EventSink`) and **`locode-exec`
streams** them to stdout as JSONL (Task 14, `--output-format stream-json`).

### In scope (v0, as built)
- `#[non_exhaustive] enum Event` (`#[serde(tag="type", rename_all="snake_case")]`):
  - `Init { session_id, harness, api_schema, model, cwd, max_turns, preamble: Vec<Message>,
    tools: Vec<Value> }` — once, first; the self-sufficiency fix.
  - `Message { message: Message }` — one full turn appended to history (the trace).
  - `Result { report: Report }` — terminal; the same `Report` as `--output-format json`.
  - `Error { message: String }` — a non-terminal note (e.g. a retry); terminal errors ride in
    `Result`.
- `reconstruct_conversation(&[Event]) -> Conversation` = `Init.preamble` ++ every `Message`
  event; `Result`/`Error` are metadata, dropped.
- The JSONL round-trip + full-history reconstruction contract (tests).

### Out of scope / deferred (reserved as `#[non_exhaustive]` additions)
- **Per-token / partial-message deltas.** The loop is non-streaming (ADR-0005), so whole-`Message`
  events suffice; deltas (cf. Claude's `--include-partial-messages`, survey
  `01-claude-code/provider-api.md`) are a future `Event` variant (ADR-0014 Alternatives).
- **Per-turn markers.** `turn.started` / `turn.completed{usage}` (cf. Codex's terminal
  `response.completed { token_usage, end_turn }`, survey `02-codex/provider-api.md:38`) are
  reserved (ADR-0014 Consequences), not built. Usage currently rides only in the terminal
  `Result.report.usage`.
- **Transcript in `json` mode.** The single `json` envelope stays a *summary*; the full trace is
  `stream-json`'s job (ADR-0014 "Transcript-in-`json`-mode: deferred").
- **On-disk JSONL session durability / replay-fork.** `reconstruct_conversation` rebuilds
  in-memory from an event slice; reading/writing durable `.jsonl` session files is deferred (SPEC
  §Assumptions 6). Grok persists to `chat_history.jsonl` for replay/fork
  (`grok …/conversation.rs:41–48` comment); we defer that.
- **Emission / sink plumbing.** The `EventSink` trait, `NullSink`/`CollectingSink`/`FnSink`, and
  the JSONL writer live in `locode-engine` (Task 6) and `locode-exec` (Task 14), not here.

---

## 2. Module layout (as built)

Task 3b is a **section appended to the Task 3 file**, `crates/locode-protocol/src/lib.rs`:

```
lib.rs
├── … Task 3 sections (Conversation, Report, ToolSpec)   lib.rs:18–244
├── ==== Streaming events (stream-json) ====             lib.rs:246–312
│   ├── enum Event  (#[non_exhaustive], tag="type")      lib.rs:256–294
│   └── fn reconstruct_conversation(&[Event]) -> …       lib.rs:296–312
└── #[cfg(test)] mod tests (shared with Task 3)          lib.rs:314–525
    ├── events_reconstruct_full_conversation             lib.rs:426–510
    └── event_uses_snake_case_type_tags                  lib.rs:512–524
```

No new module, no new file, no new dependency — `Event` reuses Task 3's `Message`/`Report` and
`serde_json::Value`. Keeping it in-crate (not a `locode-stream` crate) is deliberate: the event
protocol *is* the conversation/report protocol projected as a trajectory; splitting it would
force a circular or awkward dependency (the stream needs `Message` + `Report`, both here).

---

## 3. Key types & signatures (the actual shipped types)

Quoted verbatim from `crates/locode-protocol/src/lib.rs:256–312`.

```rust
/// One event in the `stream-json` trajectory (one JSON object per line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// Emitted once at the start — everything needed to reconstruct context.
    Init {
        session_id: String,
        harness: String,
        api_schema: String,        // the provider's api_schema() (renamed from `provider`)
        model: String,
        cwd: String,
        max_turns: u32,
        preamble: Vec<Message>,    // the base System + Developer messages
        tools: Vec<Value>,         // tool specs offered to the model, as JSON values
    },
    /// A full message appended to the history (the trace): role + content blocks.
    Message { message: Message },
    /// The terminal event: the final report (identical to `--output-format json`).
    Result { report: Report },
    /// A non-terminal error note (e.g. a retry); terminal errors ride in `Result`.
    Error { message: String },
}

/// Reconstruct the full `Conversation` from a `stream-json` event trajectory.
#[must_use]
pub fn reconstruct_conversation(events: &[Event]) -> Conversation {
    let mut messages = Vec::new();
    for event in events {
        match event {
            Event::Init { preamble, .. } => messages.extend(preamble.iter().cloned()),
            Event::Message { message } => messages.push(message.clone()),
            Event::Result { .. } | Event::Error { .. } => {}   // run metadata, not history
        }
    }
    Conversation { messages }
}
```

Serialized shape (JSONL, one object per line):

```jsonl
{"type":"init","session_id":"sess-1","harness":"grok","api_schema":"anthropic","model":"claude-opus-4-8","cwd":"/repo","max_turns":30,"preamble":[…System…,…Developer…],"tools":[{…}]}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"run echo hi"}]}}
{"type":"message","message":{"role":"assistant","content":[{"type":"tool_use","id":"c1",…}]}}
{"type":"message","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1",…}]}}
{"type":"result","report":{…schema_version:1…}}
```

---

## 4. Behavior, serde shape, and the reconstruction contract

- **Tag = `type`, snake_case.** `#[serde(tag="type")]` gives `{"type":"init",…}` /
  `{"type":"message",…}` / `{"type":"result",…}` / `{"type":"error",…}` — internally tagged so
  each line is a flat object (JSONL-friendly, no wrapper). Test `event_uses_snake_case_type_tags`
  (`lib.rs:512–524`) pins `Message → "message"`.
- **JSONL, one object per line.** The stream is newline-delimited JSON: each `Event` serializes to
  a single line; a reader parses line-by-line. The round-trip test builds the events, joins
  `serde_json::to_string(e)` with `\n`, then parses each `line` back and asserts equality
  (`lib.rs:490–500`). No array wrapper, no trailing state — a consumer can process incrementally as
  lines arrive (the streaming use case).
- **`Init` carries preamble + tools + model + cwd + max_turns.** This is the self-sufficiency
  payload: `preamble: Vec<Message>` are the base `System` + `Developer` messages (the prompt +
  capabilities), and `tools: Vec<Value>` are the tool specs. Together they are everything Claude's
  stream *omits*. `Init` is emitted **once, first**.
- **`Message` carries a full turn.** One `Message` event per appended history turn — the user turn,
  each assistant turn (text/thinking/tool_use blocks), and each tool_result-bearing user turn. The
  `tool_use`/`tool_result` blocks live *inside* the message's content, so pairing is visible in the
  trace without a separate event kind.
- **`Result` is terminal and carries the whole `Report`.** ADR-0014 "One summary, two modes": the
  terminal `result` event carries the **same** `Report` that `--output-format json` emits alone —
  so `json` and `stream-json` share one summary type. Terminal errors ride *in* `Result.report`
  (`status: error/model_error` + `error: Some(…)`), not in an `Error` event.
- **`Error` is non-terminal.** A note for a recoverable event (e.g. a provider retry; Task 6 emits
  one per resample attempt, §Task-6 5.6). It is **not** part of the reconstructed history.
- **Reconstruction = `Init.preamble` ++ every `Message`.** `reconstruct_conversation` is the
  documented inverse of what `locode-exec` emits: it seeds history with `Init.preamble` (System +
  Developer) and appends each `Message`, dropping `Result`/`Error` as run metadata. `#[must_use]`
  because the returned `Conversation` is the whole point of calling it. The full-history round-trip
  test (`lib.rs:426–510`) proves `reconstruct(parse(serialize(events)))` equals the original
  `[system, developer, user, assistant, tool_result]` conversation — **System/Developer
  included**, which is exactly the gap Claude's stream leaves open.

---

## 5. Design decisions (each: harness `file:line` · why · why-not · harness diff)

### 5.1 A self-sufficient stream: `Init` carries preamble + tools (the Claude gap fix)
- **Source.** Claude Code's `--output-format stream-json` emits only `type ∈ {user, assistant, …}`
  events wrapping a `message`; it **omits the `system` prompt and the `tools`** — "Claude Code's
  message view, without the raw system prompt" (ADR-0014 Context). The empirical capture confirms
  the base prompt lives in the top-level `system[]` and tool schemas travel in the native `tools`
  param (note surfaces 1 + "native tools param"), *neither* of which appears in the message
  stream. This forced `swe-lab` to run a second reverse-proxy capture to recover them.
- **Why.** ADR-0014 Decision: make the stream a self-sufficient, replayable source — emit the base
  prompt (`preamble`) and tool specs (`tools`) up front in `Init` so the entire run reconstructs
  with nothing else. This is the explicit "fix for Claude's gap" (lib.rs doc `:252–255`). It makes
  a saved `.jsonl` a complete artifact for replay, A/B analysis, and offline reconstruction — no
  side proxy, no second capture.
- **Why not** mirror Claude's stream (no `init`): rejected in ADR-0014 — "not self-sufficient;
  reconstruction would need a side proxy for `system`/`tools`, exactly the pain `swe-lab`
  documents."
- **Harness diff.** Claude = message-only stream (system/tools omitted); Codex = SSE `ResponseEvent`
  stream (`OutputItemDone`, `Completed{token_usage}`; survey `02-codex/provider-api.md:38`) which
  is a *provider-wire* stream, not a self-describing trajectory; Grok persists a
  `chat_history.jsonl` for replay/fork (`grok …/conversation.rs:41–48`) but that is durable session
  storage, not a live output mode. We = a self-describing JSONL trajectory with a leading `Init`.

### 5.2 JSONL, internally tagged (`#[serde(tag="type")]`), not an array or SSE
- **Source.** Claude Code's headless stream is line-delimited JSON events; Codex's wire is SSE
  `data:` lines (`process_sse_with_treatment`, survey `02-codex/provider-api.md:38`). Both are
  line-oriented, incrementally consumable.
- **Why.** ADR-0014: serialize as JSONL, one object per line, `#[serde(tag="type")]`. A flat,
  internally-tagged object per line means a consumer parses incrementally as lines arrive (the
  whole point of a *stream* mode) and every event is a self-contained line — no array bracket to
  wait for, no partial-document parsing. Internal tagging (vs adjacent/externally tagged) keeps the
  line a single flat object matching how Claude/Codex shape their events.
- **Why not** a single JSON array of events: can't be consumed incrementally (you'd wait for the
  closing `]`); defeats streaming. **Why not** SSE framing (`event:`/`data:`): that's a *transport*
  concern for a provider wire; our output contract is a file/stdout artifact, and JSONL is the
  simpler, greppable, replayable choice (matching Claude's headless format the maintainer already
  consumes).
- **Harness diff.** Codex = SSE on the wire; Claude = JSONL for its headless `-p` output (what we
  match); we = JSONL for the `stream-json` output mode.

### 5.3 `Event::Init.tools` is `Vec<Value>` while `ConversationRequest.tools` is typed `Vec<ToolSpec>`
- **Source / constraint.** `locode-tools` builds typed `ToolSpec`s (name+description+derived
  schema) and `ConversationRequest.tools: Vec<ToolSpec>` consumes them (todo.md Task 5). Codex's
  tools travel as `serde_json::Value` in some `ResponseItem`s (e.g. `AdditionalTools { tools:
  Vec<serde_json::Value> }`, `codex …/models.rs:807–811`).
- **Why.** The stream is a **trace/record**, not a re-dispatch input. `Init.tools` exists to make
  reconstruction self-sufficient — a human or analyzer reads the schemas that were offered; nobody
  re-executes tools *from the stream*. Recording them as opaque `Vec<Value>` (a) keeps the trace
  faithful to whatever JSON the pack produced (including future MCP/dynamic tools that have no
  compile-time `ToolSpec`), (b) avoids coupling the record format to the exact `ToolSpec` shape (so
  the spec can evolve without a stream-format break), and (c) matches the loop's emission path:
  Task 6 emits `registry.specs() -> Vec<Value>` straight into `Init.tools` (Task 6 §4). The typed
  `ToolSpec` is for *building the request the provider validates*; the `Value` is for *recording
  what was sent*.
- **Why not** `Init.tools: Vec<ToolSpec>`: would couple the trace format to the `ToolSpec` type
  (evolving `ToolSpec` would break saved traces) and would not accommodate raw MCP tool JSON that
  never had a typed `ToolSpec`. **Why not** put typed specs to prove they're valid: validity is the
  provider/registry's job at request time, not the trace's.
- **Harness diff.** Codex carries tool JSON as `Vec<serde_json::Value>` in trace items too
  (`models.rs:811`); we follow the same "record as opaque JSON" posture for the stream while keeping
  the *request* path typed.

### 5.4 `Result` reuses the exact `Report` — one summary, two modes
- **Source.** ADR-0009: `--output-format json` emits exactly one `Report`. ADR-0014: the terminal
  `result` event carries the *same* `Report`.
- **Why.** A single summary type means `json` mode = "the `result` event's report alone" and
  `stream-json` mode = "the full event stream ending in that same report" (ADR-0014 Consequences;
  Task 14). No divergent summary shapes, no drift between modes, and the golden-frozen envelope
  (Task 3 §4/§6) covers *both* output modes at once. Terminal errors ride in
  `Result.report.{status,error}` (not a separate terminal `Error` event), so a consumer always gets
  the structured terminal state in one place.
- **Why not** a stream-specific terminal summary distinct from the `json` report: two shapes to keep
  in sync, two golden tests, and a consumer that switches modes gets different summaries — pure
  downside.
- **Harness diff.** Claude splits `success`/error subtypes across a flat result object; Codex ends
  its wire stream with `response.completed{token_usage}` (survey `02-codex/provider-api.md:38`) but
  that is wire-level, not a run report. We end with the same `Report` both modes share.

### 5.5 `Error` non-terminal; terminal errors in `Result`
- **Source.** Grok surfaces a retryable rate-limit as terminal (not hammered) but emits transient
  notices along the way (`grok …/sampler_turn.rs` retry handling, cited in Task 6 §5.6); Codex's
  transport retries emit reconnect attempts while the *terminal* outcome is a single typed error.
- **Why.** ADR-0014: `error` is "a non-terminal note (e.g. a retry); terminal errors ride in
  `result`." This keeps a clean rule: every run ends in exactly one `Result` carrying the
  structured terminal `Status` (+ `error` string), and `Error` events are advisory breadcrumbs
  (e.g. Task 6 emits one per bounded resample attempt, §5.6). A consumer can ignore `Error` events
  entirely and still get the full outcome from `Result`.
- **Why not** make `Error` terminal: then a consumer has *two* places to find "how did it end,"
  and reconstruction would have to decide which error is fatal. Keeping terminality solely in
  `Result` is unambiguous.
- **Harness diff.** Claude folds error into the final result object; Codex separates transport
  reconnect attempts from the terminal typed error. We match: notes are `Error`, outcome is
  `Result`.

### 5.6 `#[non_exhaustive]` on `Event` — reserve deltas + turn markers
- **Source.** ADR-0014 Consequences: "Reserve turn markers (`turn.started`/`turn.completed{usage}`,
  cf. Codex) and message deltas as future `Event` variants." Codex's terminal
  `response.completed { token_usage, end_turn }` (survey `02-codex/provider-api.md:38`) is the
  model for a `turn.completed{usage}` marker; Claude's `--include-partial-messages` is the model
  for deltas.
- **Why.** The event set is the most likely thing to grow (per-turn usage markers, per-token
  deltas, tool-progress events). `#[non_exhaustive]` lets us add variants **without a breaking
  change** and forces external matchers to keep a wildcard arm — exactly the same
  additive-evolution seam as `ContentBlock` (Task 3 §5.4).
- **Why not** exhaustive: any new event kind would be semver-breaking across `engine`/`exec`.
- **Harness diff.** Both Codex (SSE event kinds) and Claude (stream event types) have rich,
  evolving event vocabularies; `#[non_exhaustive]` matches that reality while we ship the minimal
  four.

### 5.7 `Init.api_schema` (the `provider → api_schema` rename), mirrored from `Report`
- **Source.** Same rationale as Task 3 §5.9: Grok's `ApiBackend` vs `base_url`, Codex's `WireApi`
  vs `ModelProviderInfo` (survey `02-codex/provider-api.md:12`) — schema (dialect) is distinct from
  gateway (config).
- **Why.** `Init` stamps `api_schema` (the provider's `api_schema()` — the wire dialect), matching
  the `Report.api_schema` field so a reconstructed trace and its terminal report agree on
  self-describing identity. The rename landed across `Report`, `Event::Init`, ADR-0009, and the
  golden snapshot in the Task 6 timeframe (todo.md Task 6 design notes). Confirmed in the shipped
  code (`lib.rs:266–267`).
- **Why not** `provider`: conflates wire dialect with gateway/endpoint (config), defeating
  self-describing A/B (Task 3 §5.9).
- **Harness diff.** As Task 3 §5.9.

---

## 6. Tests (as built)

Shipped and green (todo.md Task 3b ✅), inline in the shared test module.

1. `events_reconstruct_full_conversation` (`lib.rs:426–510`) — the core contract. Builds a
   representative run (`Init{preamble:[system,developer], tools}`, three `Message` events
   [user, assistant-with-tool_use, user-with-tool_result], `Result{minimal_report}`), then:
   - **JSONL round-trip:** serialize each event to a line, join with `\n`, parse each line back,
     assert `parsed == events` (`:490–500`) — proves one-object-per-line losslessness.
   - **Full-history reconstruction:** `reconstruct_conversation(&parsed)` equals the original
     `Conversation { messages: [system, developer, user, assistant, tool_result] }` (`:502–509`) —
     proving the stream rebuilds the **entire** history, **System/Developer included** (the Claude
     gap, §5.1), and that `Result` is dropped as metadata.
   Uses a `minimal_report()` helper (`lib.rs:408–422`) — a `schema_version:1` `Completed` report,
   which also exercises `Event::Result` serialization.
2. `event_uses_snake_case_type_tags` (`lib.rs:512–524`) — pins `Event::Message` →
   `{"type":"message",…}` (the `#[serde(tag="type", rename_all="snake_case")]` shape). Guards §5.2.

**What is NOT tested here (by design):** no test for `Error` event reconstruction-drop
specifically (covered indirectly — `Result`/`Error` share the same drop arm); no test that
`Init.tools` opaque `Value`s survive (they're `serde_json::Value`, trivially round-tripping); no
emission-order test (that's the engine's `events_reconstruct_history`, Task 6 §6.9). Per-token
deltas / turn markers are unbuilt (§8).

---

## 7. Dependencies

No new dependencies over Task 3. `Event` reuses `Message`/`Report` (Task 3) and
`serde_json::Value`; `reconstruct_conversation` uses only `std`. `serde` (derive) + `serde_json`
already present. This remains the DAG root (`protocol ← everything`).

---

## 8. Open questions / concerns / future considerations (exhaustive & honest)

1. **Event granularity: no per-turn markers.** Usage rides only in the terminal `Result.report`;
   there is no `turn.completed{usage}` (Codex's `response.completed{token_usage}`, survey
   `02-codex/provider-api.md:38`) or `turn.started`. A live consumer watching the stream can't see
   per-turn token cost until the end. ADR-0014 reserves these as `#[non_exhaustive]` additions.
   Open: do we add `turn.started`/`turn.completed{usage}` before the first A/B (Task 16), where
   per-turn token/latency breakdown is exactly the analysis payload? Adding them also interacts
   with the usage over-count concern (Task 3 §8 Q5) — per-turn markers would make the over-count
   visible/attributable.

2. **No per-token / partial-message deltas.** The loop is non-streaming (ADR-0005), so whole-
   `Message` events suffice today. If `locode-app` (the future TUI consumer) wants live token
   rendering, it needs delta events (cf. Claude's `--include-partial-messages`; Grok's
   `TextDelta`/`ThinkingDelta`/`SignatureDelta`, `grok …/messages.rs:308–311`). Open: does adding
   deltas mean a second (streaming) loop — which SPEC §Assumptions 5 and ADR-0005 explicitly forbid
   ("streaming is an additive optimization, not a second loop") — or can whole-message events plus
   opt-in deltas coexist? The `#[non_exhaustive]` seam is there; the loop-shape implication is not
   settled.

3. **Does `reconstruct_conversation` (and the whole stream protocol) belong in `locode-protocol`,
   or a future `locode-transcript`?** `reconstruct_conversation` is the one piece of *logic* (not
   just types) in an otherwise pure-types crate. Session durability — writing/reading `.jsonl`,
   replay, fork, compaction of saved transcripts — is deferred (SPEC §Assumptions 6) but clearly
   coming; Grok has a whole `chat_history.jsonl` replay/fork machinery
   (`grok …/conversation.rs:41–48`). When that lands, should `Event` + `reconstruct_conversation`
   move to a `locode-transcript`/session crate that owns persistence and replay, leaving
   `locode-protocol` types-only? Counter-argument: the stream *is* the protocol projected as a
   trajectory, and it needs `Message`+`Report` which live here, so moving it would invert a
   dependency. This is a real seam question to settle before durability work begins.

4. **Reconstruction fidelity vs. a real transcript.** `reconstruct_conversation` faithfully rebuilds
   the *history* (preamble ++ messages), but it drops `Error` notes and `Result` — so a
   reconstructed `Conversation` loses the retry breadcrumbs and the final report. That's correct for
   "rebuild the model-visible history," but a durable session format (Q3) will want to preserve the
   `Error`/`Result` events too (for audit/replay). Do we need a second function
   (`reconstruct_trace` / a richer `Session` record) that keeps everything, or is history-only the
   right contract for this function forever?

5. **`Init` ordering / cardinality is a convention, not enforced.** The type system doesn't force
   `Init` to be first-and-once. `reconstruct_conversation` would happily prepend `preamble` from a
   *second* `Init` mid-stream (extending messages again) or produce an empty conversation if `Init`
   is missing. The loop guarantees exactly one leading `Init` (Task 6 §4), but a malformed/truncated
   stream (e.g. a crashed run) has no schema-level guard. Do we want a validating reader that
   enforces "exactly one `Init`, first; exactly one `Result`, last" — or is best-effort
   reconstruction fine for v0?

6. **`Init.preamble` vs. mid-stream `Developer` injection.** `preamble` captures the *initial*
   System + Developer messages. But ADR-0013's `Developer` role is explicitly "injected repeatedly,
   anywhere" (mid-conversation capability deltas, note surface 2). If the loop later injects a
   `Developer` message *mid-run*, it would (correctly) be emitted as a `Message` event, not folded
   into `Init.preamble` — so reconstruction still works. But it means "preamble" is a slight
   misnomer for "the base prompt at t0", and a reader can't distinguish an initial Developer message
   (in `preamble`) from a mid-run injected one (a `Message`) except by position. Is that distinction
   ever needed for analysis?

7. **`Init.tools` as opaque `Vec<Value>` loses queryability.** Recording tools as raw JSON (§5.3)
   means an analyzer must re-parse each `Value` to answer "which tools were offered / what was
   `run_terminal_command`'s schema." For A/B tool-surface comparison (survey
   `05-comparative/tool-surface.md`), a typed or at least name-indexed record might be handier. Is
   opaque JSON enough, or should `Init` also carry a lightweight `tool_names: Vec<String>` for cheap
   filtering? (Weighed against §5.3's evolvability/MCP argument.)

8. **`stream-json` output goes to stdout, but ADR-0009 says stdout is one JSON document.** ADR-0009
   Alternatives explicitly rejected "interleave events on stdout" and said "a future
   `--events-jsonl` stream must go to stderr (or a separate fd/file), never stdout." ADR-0014 then
   made `stream-json` a first-class mode where the *stream* is the stdout content (Task 14:
   "`stream-json` = the JSONL `Event` stream" on stdout). These are reconcilable — in `stream-json`
   mode the JSONL stream *is* the single well-defined stdout contract, and `json` mode keeps the
   one-document rule — but the two ADRs' wording is in mild tension and the precedence (ADR-0014
   supersedes the "events → stderr" lean) should be written down explicitly so Task 14 doesn't
   re-litigate it.

9. **Versioning: the event protocol has no `schema_version`.** The `Report` inside `Result` carries
   `schema_version:1`, but the `Event` envelope itself has none. If we add/rename event fields (even
   with `#[non_exhaustive]`, a field rename like `provider→api_schema` on `Init` *is* a shape
   change), a saved `.jsonl` from an older run may not parse. Do we need an `Init.stream_version`
   (or reuse the report's), and does the Task 3 envelope-versioning policy (Task 3 §8 Q8) extend to
   the event stream? Note the `provider→api_schema` rename on `Init` already happened without a
   version marker — same precedent, same gap.

10. **No test that `Error` events are dropped by reconstruction / survive round-trip.** The
    reconstruction test (`lib.rs:426–510`) covers `Init`/`Message`/`Result` but never constructs an
    `Event::Error`, so the `Error` arm of both `reconstruct_conversation` and the JSONL round-trip
    is unexercised. Low risk (it shares the drop arm with `Result` and is a trivial single-field
    variant), but a one-line addition would close the coverage gap.

---

### Speech-to-text / identifier confirmations
Written from shipped source + written ADRs; **no user identifiers reconstructed**. The rename to
confirm (per the task prompt) is **`provider → api_schema`** on `Event::Init`, matching the shipped
code (`lib.rs:266–267`) and `Report` (Task 3 §5.9).
