# Task 6 — `locode-engine`: the sample→dispatch→append loop + Session API

Pre-implementation plan. Source of truth: `SPEC.md`, ADR-0004 (error taxonomy + pairing),
ADR-0005 (agent loop), ADR-0007 (provider trait/retry), ADR-0014 (streaming events).
Every non-obvious decision below is grounded in the four studied harnesses with `file:line`
citations. Grok Build is the primary model for how to unify.

Submodule roots (abbreviated in citations):
- `grok` = `~/dev/coding-cli-survey/submodules/grok-build`
- `codex` = `~/dev/coding-cli-survey/submodules/codex`
- survey = `~/dev/coding-cli-survey/survey`

---

## 1. Purpose & scope

Build the headless agent spine: a `Session` that drives one run through
**sample → dispatch → append → re-sample** to one of four terminal states, emitting the
`stream-json` `Event`s (ADR-0014) as it goes and returning one `Report` (ADR-0009). This is
the highest-leverage test surface in the repo — the loop, not the tools, is where the subtle
bugs live (SPEC §Testing; plan.md Overview). It must be provable end-to-end against
`MockProvider` with **zero network** (Checkpoint B).

### In scope (v0)
- The serial, non-streaming loop (ADR-0005): buffer each assistant turn fully, then dispatch
  its tool calls serially, append results, re-sample.
- All four terminal states → `Status`: `Completed`, `MaxTurns`, `ModelError`, `Error`.
- Pre-send transcript hygiene: pairing repair + duplicate-result dedup (ADR-0004), ported from
  Grok's `repair_dangling_tool_calls` / `dedup_duplicate_tool_results`.
- Mid-batch abort synthesis: when a `Fatal` tool aborts a batch, the un-run calls in that same
  batch still get `is_error` results so the transcript stays valid.
- Faithful append/replay of `Completion.content` including `Thinking{text,signature}` blocks.
- A bounded loop-level **resample** retry tier keyed off `ProviderError::retryable()`.
- The `Session` library API + an `EventSink` seam + an `EngineConfig`.
- Emission of `Event::{Init, Message, Error, Result}` and a round-trip guarantee with
  `reconstruct_conversation`.

### Out of scope / deferred (reserved seams, not built here)
- **Parallel tool batches.** Serial-first (ADR-0005). Grok runs approved tools on a
  `FuturesUnordered` with per-file write locks (`grok …/tool_calls.rs:387-404,477`); Codex uses a
  read/write lock split (survey `02-codex/agent-loop.md:67-70`). Reserved: when added, copy
  Codex's minimal `RwLock<()>` form (ADR-0005 "Parallel tool batches in v0 — Deferred").
- **Streaming / per-token deltas.** Non-streaming buffer (ADR-0005); whole-`Message` events
  suffice (ADR-0014). No `StreamingToolExecutor` (Claude survey `01-claude-code/agent-loop.md:30`).
- **Compaction / context-overflow recovery.** Grok/Codex/Claude auto-compact mid-loop
  (survey `05-comparative/agent-loop-comparison.md:50-56`). v0 has none; `max_turns` is the only
  runaway guard (ADR-0005 "No turn cap … Rejected").
- **Doom-loop / laziness / TodoGate nudges.** Grok has an entire laziness classifier + TodoGate
  (`grok …/turn.rs:2112-2163`, `types.rs:95-178`). Explicitly not v0.
- **Permission prompts / hooks.** No interactive prompts in this repo (ADR-0001). Grok's
  `ToolLoop::{PermissionReject, HookDenied, FollowupMessage}` (`grok …/types.rs:67-89`) collapse
  away — there is no human in the loop.
- **Structured-output (`--json-schema`) interception.** Envelope-only for v0 (ADR-0014;
  SPEC Open Q3). Grok's dual native/tool path (`grok …/turn.rs:1772-1798, 2198-2228`) is a later
  milestone; `Report.structured_output` stays `None`.
- **Real external cancellation.** The `CancellationToken` is plumbed into `ToolCtx` but never
  fired in headless v0; mid-batch synthesis is exercised via the `Fatal` path, not a live cancel.
- **Transport-level retry (backoff/jitter, `Retry-After`, 401 refresh, 429 surfacing).** Lives in
  the Anthropic wire (Task 12, ADR-0007). The engine owns only the coarse loop-level resample tier.

---

## 2. Module layout (`crates/locode-engine/src/`)

> **Naming correction:** `tasks/todo.md` (Task 6 Files) lists `loop.rs`. `loop` is a Rust
> keyword — `mod loop;` is illegal without `r#loop`. Use `run.rs` (or `drive.rs`). Flagged as a
> guess for the user to confirm.

```
lib.rs        Crate docs + public re-exports (Session, EngineConfig, EventSink, sinks).
session.rs    `Session` (public driving API), constructor/builder, `run(...)`.
run.rs        The core loop: sample → dispatch → append → re-sample; the private engine driver.
terminal.rs   Internal `Terminal` outcome enum → `Status`/`Report` assembly; report accumulator.
repair.rs     Pre-send pairing repair + duplicate-result dedup (adapted from Grok). See §5.4 for
              the decision on whether this lives here or in locode-protocol.
sink.rs       `EventSink` trait + `NullSink` + `CollectingSink` (tests) + `FnSink`.
config.rs     `EngineConfig` (max_turns, resample budget, ids, sampling, cwd, jail root, …).
```

Tests: inline `#[cfg(test)]` for `repair.rs`/`terminal.rs` unit scope; the loop's terminal-state
matrix under `tests/` (integration) driving `Session` with `MockProvider` + trivial in-test tools.

Cargo deps (see §7): `locode-protocol`, `locode-tools`, `locode-provider`, `tokio`, `async-trait`,
`tokio-util`, `serde_json`. No new external crates.

---

## 3. Key types & signatures

Aligned exactly to the shipped crate types (`locode-protocol`, `locode-tools`) and the Task-5
provider surface given for this task:
`Provider { api_schema()->&str, async complete(&ConversationRequest)->Result<Completion,ProviderError> }`,
`Completion { content: Vec<ContentBlock>, usage: Usage, stop: StopReason }`,
`ConversationRequest { messages, tools, sampling_args, cache_hint }`,
`ProviderError` (exhaustive, `retryable()->bool`), `MockProvider`.

```rust
// ─── config.rs ───────────────────────────────────────────────────────────────
/// Static configuration for one session's loop. No provider/registry here.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub session_id: String,
    pub harness: String,          // stamped into Report + Init (e.g. "grok")
    pub provider: String,         // stamped into Report + Init (e.g. "anthropic" / "mock")
    pub model: String,            // for Init
    pub cwd: PathBuf,             // tool cwd
    pub workspace_root: PathBuf,  // path-jail root (ADR-0008); passed to ToolCtx
    pub max_turns: u32,           // default 30 (ADR-0005)
    pub resample_retries: u32,    // loop-level bounded resample budget; default 2 (see §5.6)
    pub sampling: SamplingArgs,   // provider-neutral knobs (locode_provider::SamplingArgs)
    pub cache_hint: CacheHint,    // reserved; wire places breakpoints (Task 12)
}
impl Default for EngineConfig { /* max_turns:30, resample_retries:2, session_id:uuid, … */ }

// ─── sink.rs ─────────────────────────────────────────────────────────────────
/// Where the loop's stream-json Events go (ADR-0014). `&mut self` so a writer sink
/// can buffer/flush; kept object-safe so locode-exec can pick a sink at runtime.
pub trait EventSink: Send {
    fn emit(&mut self, event: Event);
}
pub struct NullSink;                 // json/text output modes: drop events
pub struct CollectingSink(pub Vec<Event>); // tests: assert the stream + reconstruct
pub struct FnSink<F: FnMut(Event)>(pub F); // locode-exec wraps a JSONL stdout writer

// ─── session.rs ──────────────────────────────────────────────────────────────
/// One driven agent session. Owns history for the run (ephemeral, ADR SPEC §Assumptions 6).
pub struct Session {
    provider: Arc<dyn Provider>,     // runtime-selected by --provider (see §5.7)
    registry: Registry,              // one pack's tools, keyed by wire name (locode-tools)
    preamble: Vec<Message>,          // System + Developer messages (Init.preamble); pack supplies
    config: EngineConfig,
    sink: Box<dyn EventSink>,
    cancel: CancellationToken,       // plumbed to ToolCtx; unfired in v0
}

impl Session {
    pub fn new(
        provider: Arc<dyn Provider>,
        registry: Registry,
        preamble: Vec<Message>,
        config: EngineConfig,
        sink: Box<dyn EventSink>,
    ) -> Self { /* … cancel = CancellationToken::new() */ }

    /// Drive to a terminal state and return the Report. **Infallible**: every terminal
    /// condition (incl. Fatal + provider error) is captured *in* the Report's `status`
    /// (SPEC "callers get a structured terminal state every time"). locode-exec maps
    /// status → exit code (ADR-0009). `user` is the initial human turn's content.
    pub async fn run(&mut self, user: Vec<ContentBlock>) -> Report;

    /// Convenience: a plain-text prompt.
    pub async fn run_text(&mut self, prompt: impl Into<String>) -> Report {
        self.run(vec![ContentBlock::Text { text: prompt.into() }]).await
    }
}

// ─── terminal.rs (internal) ──────────────────────────────────────────────────
/// Why the loop stopped. Maps 1:1 onto protocol `Status`.
enum Terminal {
    Completed { final_message: Option<String> }, // no tool_use in assistant turn
    MaxTurns,                                     // ceiling hit after a dispatch batch
    ModelError { error: String },                 // provider err after bounded resample
    Error { error: String },                      // a Dispatched.fatal aborted the batch
}
impl Terminal { fn status(&self) -> Status { /* Completed|MaxTurns|ModelError|Error */ } }

/// Accumulates report-side state across turns.
struct RunAcc {
    turns: u32,
    tool_calls: Vec<ToolCallRecord>,
    usage: Usage,
    last_assistant_text: Option<String>,
}
```

`run(...)` builds the report at the end from `RunAcc` + `Terminal` + `config`.

---

## 4. The loop algorithm — step by step (every edge case)

Naming: **sample** = one `provider.complete`; **batch** = the tool_use blocks of one assistant
turn; **turn** = one sample→dispatch cycle. `turns` in the Report counts samples performed.

```
run(user_content):
  # ---- setup ----
  history = preamble.clone()                       # System + Developer
  emit Init{ session_id, harness, provider, model, cwd, max_turns,
             preamble = preamble.clone(),
             tools = registry.specs() -> Vec<Value> }          # ADR-0014 self-sufficiency
  user_msg = Message{ role: User, content: user_content }
  history.push(user_msg); emit Message{user_msg}
  acc = RunAcc::default()

  # ---- main loop ----
  loop:
    # (a) PRE-SEND HYGIENE — unconditional, before EVERY sample (ADR-0004, ADR-0007)
    repair::pairing(&mut history)         # synth is_error for dangling tool_use; dedup dup results

    # (b) SAMPLE with bounded loop-level resample (ADR-0005 "after bounded retry"; §5.6)
    request = ConversationRequest{ messages: history.clone(),
                                   tools: registry.specs(), sampling, cache_hint }
    completion = match sample_with_retry(request):        # see §5.6
        Ok(c)  => c
        Err(e) => { terminal = ModelError{ e.to_string() }; break }   # non-retryable OR budget spent
    acc.turns += 1
    acc.usage += completion.usage                          # accumulate (see §5.8)

    # (c) APPEND the assistant turn VERBATIM — Thinking/Text/ToolUse all preserved (§5.5)
    assistant_msg = Message{ role: Assistant, content: completion.content }
    history.push(assistant_msg); emit Message{assistant_msg}
    acc.last_assistant_text = join_text_blocks(&assistant_msg)   # for final_message / MaxTurns

    # (d) EXTRACT tool_use blocks in document order
    calls = assistant_msg.content.iter().filter_map(ToolUse)     # (id, name, input)

    # (e) TERMINAL: no tools ⇒ Completed  (grok turn.rs:2112,2191; claude survey:47)
    if calls.is_empty():
        terminal = Completed{ final_message: acc.last_assistant_text.clone() }
        break
        # NOTE stop == Refusal/ContentFilter with empty content: v0 still returns Completed
        # (final_message = None). Grok emits a provider-refusal notice chunk
        # (grok turn.rs:2092-2111); we defer that UX nicety — see §8 Open Q.

    # (f) DISPATCH the batch SERIALLY, collecting paired tool_results
    results: Vec<ContentBlock> = []
    fatal: Option<String> = None
    for (idx, call) in calls.enumerate():
        if fatal.is_some():
            # (f.1) MID-BATCH ABORT SYNTHESIS: a prior call was Fatal — do NOT run this one,
            # but still pair it so the transcript is valid (ADR-0004; grok
            # repair_dangling reason=HarnessHalted). Synthesize an is_error result.
            results.push(synthetic_error_result(call.id,
                "tool not executed: a prior tool in this batch aborted the turn"))
            continue
        ctx = ToolCtx::new(cwd, call.id.clone(), workspace_root, cancel.clone())
        dispatched = registry.dispatch(&call.name, call.input, &ctx).await  # locode-tools door
        results.push(dispatched.tool_result)          # ALWAYS paired, even on fatal (Dispatched)
        acc.tool_calls.push(dispatched.record)        # report view
        if let Some(msg) = dispatched.fatal:          # ToolError::Fatal → abort after appending
            fatal = Some(msg)

    # (g) APPEND the tool_result batch as ONE User message (Anthropic shape)
    tool_msg = Message{ role: User, content: results }
    history.push(tool_msg); emit Message{tool_msg}

    # (h) TERMINAL: Fatal ⇒ Error (transcript already valid — batch fully paired in (f))
    if let Some(msg) = fatal:
        terminal = Error{ error: msg }
        break

    # (i) TERMINAL: MaxTurns — checked AFTER dispatch, like Grok (grok turn.rs:2288-2298)
    if acc.turns >= max_turns:
        terminal = MaxTurns
        break
    # else continue → next sample (loops back to (a): pre-send hygiene runs again)

  # ---- assemble + emit ----
  report = build_report(terminal, acc, config)   # status, final_message, turns, tool_calls, usage, error
  emit Result{ report.clone() }
  return report
```

### Edge-case ledger (each condition and how it is handled)

| Condition | Handling | Terminal | Cite |
|---|---|---|---|
| Assistant turn has no `ToolUse` | Completed; `final_message` = joined Text of that turn | Completed | grok turn.rs:2112-2196; claude survey:47 |
| Assistant turn has `ToolUse` blocks | dispatch serially, append results, re-sample | (continue) | grok turn.rs:2229-2260 |
| A tool returns `ToolError::Respond` | `Dispatched` already made `is_error` result; loop continues | (continue) | ADR-0004; codex `RespondToModel` fn_call_error.rs:6 |
| A tool returns `ToolError::Fatal` | append its paired `is_error` result + synth results for the rest of the batch, then stop | Error | ADR-0004; codex `Fatal` fn_call_error.rs:8; grok types.rs |
| Second+ call in a batch after a Fatal | not executed; synthesize `is_error` result (pairing held) | Error | grok `synthetic_dangling_result_text` conversation.rs:2885 |
| `provider.complete` → `Err`, `retryable()==true`, budget left | backoff, resample same history | (continue) | ADR-0007; codex responses_retry.rs:48-76 |
| `provider.complete` → `Err`, `retryable()==false` OR budget spent | stop | ModelError | ADR-0005; grok RateLimited-terminal sampler_turn.rs:628-644 |
| `acc.turns >= max_turns` after a dispatch batch | stop; transcript valid (batch paired) | MaxTurns | grok turn.rs:2288-2298 |
| History arrives with a dangling `tool_use` (defense-in-depth) | pre-send `repair::pairing` synthesizes results | (continue) | grok `repair_dangling_tool_calls` conversation.rs:2784 |
| Duplicate `tool_result` for one id | pre-send dedup keeps the last | (continue) | grok `dedup_duplicate_tool_results` conversation.rs:2911 |
| `Completion.content` has `Thinking{signature}` | appended verbatim in the Assistant message; replayed on next sample | (continue) | claude survey:107; §5.5 |
| Unknown tool name / bad args | `Registry::dispatch` already returns a soft `is_error` result (not fatal) | (continue) | locode-tools registry.rs:197-227 |

### `stop: StopReason` mapping

The loop keys the **structural** decision (continue vs Completed) off *presence of tool_use
blocks*, exactly as Grok/Codex/Claude do — **not** off `stop`. This is deliberate: the tool_use
blocks are the ground truth; `stop` is advisory and can be a catch-all `Unknown` on new servers
(grok keeps a `StopReason::Unknown(String)` last variant precisely so an unknown value never
fails the parse, `grok …/messages.rs:219-225`). `stop` is used only for:
- `Length`/`MaxTokens`: a truncated assistant turn. v0 appends what arrived and proceeds (a
  truncated turn with no tool_use → Completed with partial text). Recovery/escalation
  (Claude does ≤3 max-output-token retries, survey:53) is **deferred**.
- `Refusal`/`ContentFilter` with empty content: Completed, `final_message = None` (see §8 Open Q).

Grok's canonical `StopReason` = `{ Stop, Length, ToolCalls, ContentFilter }`
(`grok …/conversation.rs:606-615`); the exact Task-5 shape is an §8 open question.

---

## 5. Design decisions (each: harness `file:line` · why · why-not-alternative · how harnesses differ)

### 5.1 Sample-then-dispatch, not tools-in-stream
- **Source.** Grok samples to done then `execute_tool_calls` (`grok …/turn.rs:2260`); Codex
  records `FunctionCall` items then drains tool futures (`codex …/session/turn.rs:1908`
  `drain_in_flight`, survey `02-codex/agent-loop.md:43-54`); Claude has a hybrid streaming
  executor but is "still conceptually Pattern A" (survey `05-comparative/agent-loop-comparison.md:14`).
- **Why.** ADR-0005: for a JSON-output headless engine, in-stream execution buys nothing (tool
  work overlapping token generation is irrelevant) and costs a 15+-event reconstruction plus an
  Effect/Promise bridge (OpenCode's Pattern B tax, `05-comparative/…:16`).
- **Why not Pattern B.** Rejected in ADR-0005 — large event-stream reconstruction cost for a
  benefit we don't use.
- **Harness diff.** OpenCode alone runs tools inside `streamText` with a hand-written outer
  `while(true)` (`05-comparative/…:9-16`); the other three (and us) execute after the sample.

### 5.2 Serial dispatch in v0
- **Source.** Grok parallelizes on `FuturesUnordered` (`grok …/tool_calls.rs:477`) with per-file
  write mutexes keyed by a single path arg (`…/tool_calls.rs:387-404`) — and that keying
  explicitly does **not** cover multi-file/`apply_patch` ops (survey `03-grok-build/agent-loop.md:68`).
  Codex uses a read/write lock split (survey `02-codex/agent-loop.md:67-70`).
- **Why.** ADR-0005: correctness before speed; a serial loop has no file-race surface to get
  wrong. The parallel form is a reserved seam (copy Codex's `RwLock<()>` when it lands).
- **Why not parallel now.** The write-lock keying is subtle and pack-specific (Grok's own lock
  misses `apply_patch`); shipping it before the tools exist invites silent file races.
- **Harness diff.** Grok = per-path mutex map; Codex = one read/write lock; Claude = streaming
  executor ordering; we = plain `for` loop.

### 5.3 Four terminal states, keyed off tool-use presence + fatal + budget + ceiling
- **Source.** Grok's `TurnOutcome { Completed, Cancelled, MaxTurnsReached }` (`grok …/types.rs:43-65`)
  and `ToolLoop { Continue, …, HookDenied, … }` (`…/types.rs:67-89`); the max-turns check is
  **not** a `ToolLoop` variant — it lives in the outer loop *after* `execute_tool_calls`
  (`grok …/turn.rs:2288-2298`). Claude's transition set is richer:
  `completed, model_error, aborted_*, max_turns, …` (claude survey:83).
- **Why.** ADR-0005 fixes exactly four: no tools → `Completed`; `Fatal` → `Error` (non-zero);
  provider error after bounded retry → `ModelError`; ceiling → `MaxTurns`. This is the minimal
  set that still yields a structured terminal every time. We collapse Grok's `Cancelled`/
  permission/hook variants because there is no human in this headless loop (ADR-0001).
- **Why not Codex's no-cap model.** Codex trusts compaction as the only runaway guard and has
  *no* `max_turns` (survey `02-codex/corner-cases.md`; comment `codex …/turn.rs:359`). Rejected in
  ADR-0005: v0 has no compaction, so a ceiling is the simpler safe guard.
- **Harness diff.** Codex = unbounded + compaction; Grok/Claude/OpenCode = explicit ceiling (we
  match the latter).

### 5.4 Max-turns checked AFTER the dispatch batch
- **Source.** Grok: `next_turn = tool_turn_count + 1; if next_turn > limit { return MaxTurnsReached }`
  evaluated after `execute_tool_calls` returns (`grok …/turn.rs:2288-2298`). Claude:
  `nextTurnCount = turnCount + 1; if maxTurns && nextTurnCount > maxTurns → max_turns`, also after
  tools (claude survey:70-71).
- **Why.** Checking after dispatch means the ceiling terminates a run whose transcript is already
  valid (the batch's results are appended and paired), so `MaxTurns` never leaves a dangling
  tool_use. It also counts "productive" turns (a turn that did tool work) rather than cutting off
  before the model can use a result.
- **Why not check before sampling.** Would either waste the final sample or risk cutting between a
  `tool_use` and its `tool_result`, violating pairing.
- **Harness diff.** Grok and Claude both post-check with `+1 > limit`; identical shape.

### 5.5 Append `Completion.content` verbatim — Thinking blocks and all — and replay it
- **Source.** Grok appends every response item: `ConversationItem::Assistant(_) → record_assistant_response`,
  others `push_tool_result` (`grok …/turn.rs:2069-2078`). Claude's `query.ts` comments stress
  "thinking blocks must stay contiguous with `tool_use` → `tool_result` → next assistant" and that
  fallback paths strip signature blocks *carefully* to avoid breaking the prompt cache
  (claude survey:107).
- **Why.** Our `Completion.content: Vec<ContentBlock>` already carries `Thinking{text,signature}`,
  `Text`, and `ToolUse` in order. Wrapping it as `Message{role: Assistant, content: completion.content}`
  and pushing it unchanged preserves the exact block order + the opaque `signature` the provider
  needs to replay extended thinking on the next request. No re-ordering, no dropping thinking.
- **Why not synthesize a text-only assistant message.** Would drop `signature`, breaking thinking
  replay and prompt-cache continuity, and would desync the trace from what the model actually
  emitted.
- **Harness diff.** All Pattern-A harnesses keep the assistant items intact; only *fallback*
  paths (streaming failure) strip signatures, which v0 (non-streaming) never hits.

### 5.6 Bounded loop-level resample tier, keyed off `ProviderError::retryable()`
- **Source.** Two tiers exist in Grok/Codex. **Transport tier** (inside the wire): Codex's
  `handle_retryable_response_stream_error` retries with `backoff(retry_count)`, honors a
  server-requested delay (`CodexErr::Stream(_, requested_delay)`, i.e. `Retry-After`), and falls
  back WS→HTTPS before giving up (`codex …/responses_retry.rs:22-79`). **Higher tier** (in the
  turn loop): Grok's `handle_sampling_failure` treats `RateLimited` as terminal — surfaced, not
  hammered (`grok …/sampler_turn.rs:628-644`) — `encrypted_content` 400 as terminal
  (`…:609-627`), and 401 as a single auth-refresh-and-resubmit (`…:645-720`); its
  `AuthRetrySchedule` is 1s/2s/4s, max 3 (`grok …/turn.rs:2323-2350`).
- **Why.** ADR-0007 mandates "two-tier retry (transport backoff+jitter honoring `Retry-After`;
  loop-level rebuild-and-resample, bounded); surface 429s; treat context-overflow and quota as
  terminal." The **engine owns only the loop-level tier**: on `Err(e)`, if `e.retryable()` and the
  budget (`config.resample_retries`, default 2) is not spent, sleep a bounded backoff and
  re-sample the rebuilt request; otherwise → `ModelError`. `retryable()` is the exhaustive
  classifier that decides — 429/quota/context-overflow return `false` (terminal), transient 5xx/
  network return `true`. The transport tier + `Retry-After` + 401 refresh belong to the wire
  (Task 12), which the engine can't reach without a network.
- **Why not put all retry in the engine.** The engine is provider-agnostic and network-free;
  `Retry-After` headers, WS/HTTPS fallback, and token refresh are wire concerns. Duplicating them
  in the engine would couple it to a wire. Why not zero engine-level retry: a bounded resample is
  the ADR-0007 contract and is the only tier `MockProvider` can exercise for the `ModelError`
  test (a scripted `Err` that resolves after N attempts, or never).
- **Harness diff.** Codex's transport retry lives in `responses_retry.rs`; Grok splits transport
  (sampler actor) from higher-level recovery (`handle_sampling_failure` / `run_turn_via_sampler`
  `grok …/sampler_turn.rs:860-915`). We mirror the split: wire = transport, engine = bounded
  resample.

```rust
async fn sample_with_retry(&mut self, request: ConversationRequest)
    -> Result<Completion, ProviderError>
{
    let mut attempt = 0;
    loop {
        match self.provider.complete(&request).await {
            Ok(c) => return Ok(c),
            Err(e) if e.retryable() && attempt < self.config.resample_retries => {
                attempt += 1;
                self.sink.emit(Event::Error {                     // ADR-0014: non-terminal note
                    message: format!("provider error (retry {attempt}/{}): {e}",
                                     self.config.resample_retries) });
                tokio::time::sleep(backoff(attempt)).await;        // bounded; jitter optional
                // (request unchanged: history didn't advance — a pure resample)
            }
            Err(e) => return Err(e),                               // terminal → ModelError
        }
    }
}
```

### 5.7 `Arc<dyn Provider>` + `Box<dyn EventSink>` (runtime selection)
- **Why.** `locode-exec` selects the provider by `--provider {anthropic,mock}` and the output
  mode by `--output-format {json,text,stream-json}` (ADR-0014; todo Task 14) at runtime, so both
  must be trait objects. `Provider` is object-safe (single `async fn` via `async-trait`, like
  `locode-tools::DynTool`). `Registry` is already a concrete type holding `Box<dyn DynTool>`.
- **Why not generics `Session<P: Provider, S: EventSink>`.** Monomorphization gives nothing here
  and forces the binary to enumerate provider×sink combinations. Trait objects keep `run` in one
  place; the mock tests just pass `Arc::new(MockProvider…)`.
- **Harness diff.** Grok/Codex both hide the concrete sampler behind an actor/handle
  (`grok …/sampler_turn.rs`); dynamic dispatch is the norm.

### 5.8 Pre-send hygiene: adapt Grok's `repair_dangling_tool_calls` + `dedup_duplicate_tool_results`
- **Source.** `grok …/conversation.rs:2784` (`repair_dangling_tool_calls`), `:2854`
  (`has_dangling_tool_calls`), `:2885` (`synthetic_dangling_result_text` with
  `DanglingToolCallReason::{UserCancelled, HarnessHalted{class}}` at `:2764`), `:2911`
  (`dedup_duplicate_tool_results`, keeps the **last** result per id). The API rejects an
  unanswered `tool_use` ("No tool output found for function call …") or a duplicated result
  ("each tool_use must have a single result") — the comments say so verbatim.
- **Why.** ADR-0004: pairing is a *wire-format* invariant; enforce it as **one function called
  unconditionally before every send**, not scattered checks. Our loop constructs paired
  transcripts by design (§4 f–g), so `repair::pairing` is a belt-and-suspenders guard that only
  fires on caller-supplied history or a future streaming-abort path — but running it every
  iteration is the ADR-0004 posture and costs one linear scan.
- **Structural adaptation (Grok → us).** Grok's history is a flat `Vec<ConversationItem>` with
  `Assistant` and `ToolResult` as sibling items; ours nests `ToolUse` inside an `Assistant`
  `Message` and `ToolResult` inside the immediately-following `User` `Message`(s). So the port:
  - Scan `Vec<Message>`. For each `Assistant` message, collect its `ToolUse` ids in order.
  - Gather answered ids from `ToolResult` blocks in the run of following `User` messages.
  - Append synthetic `ToolResult{is_error:true}` blocks (into the following `User` message, or a
    new one) for any unanswered id, preserving order — the same phase-1-scan / phase-2-apply shape
    as `grok …/conversation.rs:2788-2843`.
  - Dedup: within that following-results run, keep the last `ToolResult` per `tool_use_id`
    (`grok …/conversation.rs:2931-2957`).
- **Where it lives (decision + open Q).** ADR-0004 wants ONE function; ADR-0007/Task 12 wants the
  **wire** to call it before every request too. Provider must not depend on engine (SPEC dep
  graph). The only crate both `engine` and every `provider` wire share is `locode-protocol`.
  **Recommendation:** add `repair_pairing(&mut Vec<Message>) -> RepairStats` (plus the two
  primitives) to `locode-protocol` — additive `pub fn`s on an already-shipped crate, reusable by
  engine (each iteration) and each wire (pre-serialize). This is additive (no signature/envelope
  change), so it does not trip the "Ask first" boundary — but it does touch a shipped crate, so
  flagged in §8. If the user prefers, keep it in `engine/repair.rs` for v0 and re-home it in
  Task 12; the loop behaves identically either way.
- **Harness diff.** Grok exposes reusable repair+dedup+`has_dangling` helpers; Codex instead
  records outputs in an ordered `drain_in_flight` and, on interrupt, writes a single
  `TurnAborted` INTERRUPTED_GUIDANCE marker (`codex …/tasks/mod.rs:103`) rather than per-call
  synthesis; Claude "synthesize[s] missing tool_results" on mid-stream abort (claude survey:42-44).
  We follow Grok's explicit-synthesis model because it is the cleanest fit for a paired-by-id
  transcript and is exactly what ADR-0004 cites.

### 5.9 Usage accounting = summation, cost deferred
- **Source.** ADR-0014: "stay tokens-only for now (like Codex); `total_cost_usd` is a TODO."
- **Why.** Accumulate `Usage` (input/output/cache) across completions into `Report.usage`. Note a
  known nuance: each request re-sends the full history, so summing `input_tokens` across turns
  over-counts context; v0 accepts this (the envelope is a summary; precise per-request accounting
  and cost land with a pricing table later). Alternative considered: take input/cache from the
  *final* completion only (reflects the full context once) and sum output. Recommend plain
  summation for v0 simplicity, with the caveat documented; confirm in §8.

---

## 6. Tests (mock-provider scripts; zero network)

`MockProvider` (Task 5) returns a scripted `Vec<Completion>` in order; trivial in-test tools
(`Echo` soft-success, `Boom` → `ToolError::Fatal`, mirroring `locode-tools` test tools) register
into a `Registry`. A `CollectingSink` captures the event stream.

**Terminal-state matrix (one test each):**
1. `completed_no_tools` — script `[Completion{content:[Text], stop:Stop}]` → `Status::Completed`,
   `final_message == Some("…")`, `turns == 1`, `tool_calls empty`. Events:
   `Init, Message(user), Message(assistant), Result`.
2. `tool_then_complete` — `[Completion{[ToolUse echo]}, Completion{[Text]}]` → `Completed`,
   `turns == 2`, one `ToolCallRecord{ok:true}`. Assert history + event order:
   `Init, Msg(user), Msg(assistant#1 w/ tool_use), Msg(user tool_result), Msg(assistant#2), Result`.
3. `max_turns` — provider always returns `[ToolUse echo]`; `max_turns == 2` → `Status::MaxTurns`,
   `turns == 2`, transcript valid (every batch paired). Asserts the post-dispatch check
   (§5.4) and that MaxTurns leaves no dangling tool_use.
4. `model_error_after_bounded_retry` — provider returns `Err(retryable)` every time;
   `resample_retries == 2` → 3 attempts then `Status::ModelError`, `error` set, and **two**
   `Event::Error` retry notes emitted. Plus `model_error_non_retryable` — `Err(retryable=false)`
   → immediate `ModelError`, zero retry notes.
5. `fatal_tool_error` — `[Completion{[ToolUse boom]}]`, boom → `Fatal` → `Status::Error`,
   `error == "…"`, the boom `tool_result{is_error}` present and paired (transcript valid), loop
   stops (no re-sample).

**Transcript-validity / hygiene:**
6. `mid_batch_abort_synthesis` — one assistant turn with **two** tool_use blocks `[boom, echo]`;
   serial dispatch runs boom (Fatal) and must **not** run echo, yet the appended `User` message
   carries **two** `ToolResult`s: boom's `is_error` + a synthesized `is_error` for echo. Status
   `Error`. This is the mid-batch-abort requirement (ADR-0004; §4 f.1). *(A symmetric variant
   `[echo, boom]` confirms order-independence.)*
7. `repair_pairing_unit` (in `repair.rs`, ported from Grok's tests) — (a) a history ending in an
   assistant `tool_use` with no following `tool_result` → repair synthesizes one `is_error`
   result; (b) two `tool_result`s for one id → dedup keeps the last; (c) a fully-paired history is
   unchanged (idempotent). Mirrors `grok …/conversation.rs` repair/dedup semantics.

**Replay / stream fidelity:**
8. `thinking_block_replayed` — `Completion{content:[Thinking{text,signature:Some}, Text, ToolUse]}`;
   assert the appended `Assistant` message preserves the `Thinking` block with its `signature`
   verbatim, and that the *next* `ConversationRequest.messages` (captured via a spy provider or by
   inspecting `history`) still contains it. Guards §5.5.
9. `events_reconstruct_history` — after any multi-turn run, `reconstruct_conversation(&sink.0)`
   equals the engine's final `history` (ADR-0014 round-trip). Confirms `Init.preamble` +
   `Message` events are self-sufficient and `Result`/`Error` events are metadata.
10. `report_golden_shape` (optional, light) — a fixed scripted run serializes to a stable
    `Report` (the envelope golden already exists in `locode-protocol`; here just assert
    `schema_version==1`, `harness`/`provider` stamped, `status` correct).

---

## 7. Dependencies to add

No new **external** crates (no "Ask first" trigger). `crates/locode-engine/Cargo.toml` gains the
workspace-internal deps + already-vendored async stack:

| Dep | Why |
|---|---|
| `locode-protocol` | `Message`, `ContentBlock`, `Event`, `Report`, `Status`, `Usage`, `reconstruct_conversation` (+ `repair_pairing` if hosted here per §5.8). |
| `locode-tools` | `Registry`, `Dispatched`, `ToolCtx`, `ToolSpec`. |
| `locode-provider` | `Provider`, `Completion`, `ConversationRequest`, `SamplingArgs`, `CacheHint`, `StopReason`, `ProviderError`, `MockProvider` (dev-dep for tests). |
| `tokio` | runtime + `tokio::time::sleep` for the resample backoff. |
| `async-trait` | `EventSink` stays sync, but `Provider`/`DynTool` are `async_trait`; already a workspace dep. |
| `tokio-util` | `CancellationToken` to build `ToolCtx` (matches `locode-tools`). |
| `serde_json` | `registry.specs()` → `Vec<Value>` for `Event::Init.tools`. |

Per SPEC dep graph, `engine → packs + tools + provider + host + protocol`; Task 6 needs only
`protocol + tools + provider` (mock + trivial tools). `packs`/`host` wiring arrives with Task 9+.

---

## 8. Open questions (need user confirmation)

1. **`repair_pairing` home (§5.8).** Recommendation: add the repair+dedup `pub fn`s to
   `locode-protocol` (additive; shared by engine + every wire, satisfying ADR-0004's "one
   function" and ADR-0007/Task-12's "wire calls it before every send"). Acceptable to instead keep
   them in `engine/repair.rs` for v0. Which?
2. **Task-5 `StopReason` shape.** The loop keys off tool-use presence, not `stop`, but needs the
   enum to exist. Propose `#[non_exhaustive] enum StopReason { EndTurn, ToolUse, MaxTokens,
   Refusal }` (neutral, Anthropic-leaning) with the wire mapping Grok's canonical
   `{Stop, Length, ToolCalls, ContentFilter}` (`grok …/conversation.rs:606`). Confirm the variant
   set (this is really a Task-5 decision that Task 6 consumes).
3. **Refusal/empty-content turns.** v0 treats "no tool_use" as `Completed` even when
   `stop == Refusal`/`ContentFilter` (`final_message = None`). Grok emits a provider-refusal notice
   (`grok …/turn.rs:2092-2111`). OK to defer that notice, or should a refusal map to a distinct
   report signal now?
4. **`resample_retries` default.** Propose `2` (3 total attempts). Grok's analogous per-incident
   auth schedule is max 3 (`grok …/turn.rs:2329`); its completion-requirement recovery is
   config-driven (`…/turn.rs:1418`). Confirm default (and whether backoff needs jitter in v0).
5. **`run` signature.** Propose infallible `async fn run(&mut self, user: Vec<ContentBlock>) -> Report`
   (+ `run_text`) — all terminals captured in `Report.status`, exec maps status→exit (ADR-0009).
   Confirm vs `Result<Report, EngineError>`.
6. **Provider/sink dispatch.** Propose `Arc<dyn Provider>` + `Box<dyn EventSink>` (runtime
   selection, §5.7). Confirm vs generics.
7. **Module filename.** `todo.md` says `loop.rs`; `loop` is a keyword. Using `run.rs`. Confirm.
8. **Usage summation nuance (§5.9).** Sum all fields (simple, over-counts input across turns) vs
   sum-output / last-input. Propose plain summation with a documented caveat for v0.
9. **`final_message` extraction.** Propose: join the terminal assistant turn's `Text` blocks with
   `\n`. For `MaxTurns`, `final_message` = last assistant text (may be `None`). Confirm.

---

## 9. Speech-to-text / identifier confirmations

No user identifiers were reconstructed for this planning task (it worked from written ADRs/source).
The one **spec-vs-keyword** correction I am making without asking: the module `loop.rs` →
`run.rs` (Open Q7), because `mod loop;` does not compile.
