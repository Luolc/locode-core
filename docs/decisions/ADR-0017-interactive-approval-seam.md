# ADR-0017: Interactive approval seam at the engine's dispatch step

## Status
Proposed (under review)

## Date
2026-07-20

## Context
ADR-0008 deliberately ruled interactive approval "out of scope by ADR-0001
(headless; no human to prompt)" — correct for the headless core, but the planned
TUI app needs tool calls to pause for a user decision. The core must stay
headless *and* offer the seam; the prompt UI itself lives in the app.

The engine's dispatch site today: `dispatch_batch`
(`crates/locode-engine/src/run.rs:136-168`) iterates a turn's calls serially,
builds a `ToolCtx` (`run.rs:151-156`), and calls
`Registry::dispatch(&name, input, &ctx)`
(`crates/locode-tools/src/registry.rs:196`). It already contains the machinery a
deny path needs: `synthetic_error()` (`run.rs:288-296`) pairs an un-run
`tool_use` with an `is_error` result, keeping the transcript valid (ADR-0004).

**All four studied harnesses use the same mechanism** — the agent's tool future
suspends on a one-shot reply while the UI stays responsive and queues the prompt:

- **Grok Build** — the core sends an ACP `RequestPermissionRequest` whose
  `response_tx` oneshot the TUI resolves later; the TUI *enqueues* requests
  (FIFO per agent) and never blocks its loop
  (`xai-grok-pager/src/app/acp_handler/permissions.rs:20-89`; decision send-back
  at `app/dispatch/permissions.rs:110-119`). Notably, **YOLO/always-allow is
  implemented client-side** — the TUI auto-answers `AllowOnce` without user
  interaction (`acp_handler/permissions.rs:49-65`).
- **Claude Code** — a `canUseTool` callback returns a Promise resolved by the UI
  (`src/hooks/useCanUseTool.tsx:28-32`); policy returns `allow`/`deny`/`ask` and
  only `ask` reaches the dialog queue
  (`src/hooks/toolPermission/handlers/interactiveHandler.ts:57`); the loop's
  `await` inside `runTools` (`src/query.ts:1382`) suspends just that tool.
- **Codex** — approval requests arrive as server requests the TUI resolves or
  rejects (`tui/src/app_server_session.rs:1171-1187`), rendered from a FIFO
  queue, one at a time (`bottom_pane/approval_overlay.rs:591`).
- **opencode** — `permission.asked` events + an HTTP reply endpoint with
  `once | always | reject` (`packages/protocol/src/groups/permission.ts:119`);
  "auto" mode replies `once` automatically in the client
  (`packages/tui/src/context/sync.tsx:191-198`).

Convergent lessons: (1) the decision point suspends **only the tool call**, via
an awaited reply; (2) prompt queueing and **stickiness ("always allow") live in
the frontend**, not the core; (3) deny produces an error *result* the model
sees, not a crashed run.

## Options considered

### Where the seam hooks

**Option P1 — engine level, in `dispatch_batch` before `Registry::dispatch`
(RECOMMENDED).** The engine consults an injected decider per call; deny short-
circuits to a `synthetic_error`-style paired result.

- Pros: the pairing invariant stays where it is already enforced (`run.rs:144-149`
  handles exactly this "not executed, still paired" shape today); the tools
  crate stays interaction-free — important because `Registry` is a first-class
  library surface for downstream consumers running *their own* loop (SPEC
  Users #4), who must not inherit an approval dependency; one hook works for
  every pack.
- Cons: the decider sees the pre-dispatch view (`name`, raw `input`, `ToolKind`)
  — not host-resolved detail (e.g. the jail-resolved absolute path). Acceptable
  for v1; the studied UIs also render from the request args (grok's
  `build_permission_display`, `acp_handler/permissions.rs:227-355`).

**Option P2 — inside `Registry::dispatch` (the one door, ADR-0008).**
- Pros: literally "policy at the door."
- Cons: puts an async user-interaction await into `locode-tools`, coupling the
  host-agnostic tool framework to frontend concerns and forcing every
  tool-surface-only consumer to thread an approver. ADR-0008's actual lesson is
  *don't scatter policy in tools* — an engine-level gate before the door honors
  that without contaminating the framework crate. Rejected.

**Option P3 — event-based (emit an approval event, await a response channel).**
- Cons: `EventSink::emit` is fire-and-forget by design
  (`crates/locode-engine/src/sink.rs:7-10`); making one event type secretly
  require a reply inverts ADR-0014's contract and would deadlock every existing
  headless sink (`NullSink`, the JSONL writer). Rejected.

### The decision vocabulary

**Option V1 — minimal: `Allow` | `Deny { reason }` (RECOMMENDED).** Stickiness
("always allow this command", YOLO mode) is the approver *implementation's* job,
client-side — exactly where grok (`permissions.rs:49-65`) and opencode
(`sync.tsx:191-198`) put it. Keeps the core contract tiny and stable
(ADR-0015's trait-stability posture).

**Option V2 — sticky decisions in the core (`AllowAlways{scope}` …).** Requires
the core to define scoping/persistence semantics every frontend must share.
Deferrable additively (new enum variants) if a real consumer needs it. Rejected
for v1.

**Option V3 — ACP-style server-defined option lists (grok's wire shape).**
Right for a *protocol between processes*; overweight for an in-process trait
seam. The TUI can present richer choices and map them down to V1. Rejected.

## Decision (proposed)

1. A new engine-level trait (in `locode-engine`, re-exported by the facade):

   ```rust
   #[async_trait]
   pub trait Approver: Send + Sync {
       async fn decide(&self, request: &ApprovalRequest<'_>) -> Decision;
   }

   pub struct ApprovalRequest<'a> {
       pub tool_use_id: &'a str,
       pub tool_name: &'a str,
       pub kind: Option<ToolKind>,   // Shell/Read/Write/Edit… (tool.rs:36)
       pub input: &'a serde_json::Value,
   }

   pub enum Decision { Allow, Deny { reason: String } }
   ```

   `kind` lets an approver auto-allow read-only tools without knowing tool
   names — the `ToolKind` taxonomy already exists on the registry side.

2. **Injection:** `Session::with_approver(Arc<dyn Approver>)` builder-style
   setter — `Session::new`'s five-argument signature stays intact (public-API
   stability, ADR-0015 posture). Default: a built-in `AllowAll`, so `locode-exec`
   and all existing consumers are byte-for-byte unchanged in behavior.

3. **Hook point:** in `dispatch_batch` (`run.rs:143`), before constructing
   `ToolCtx`: `Deny { reason }` pushes a paired `is_error` tool_result carrying
   the reason (via the existing `synthetic_error` shape, `run.rs:288`) and
   records it in `acc.tool_calls`; the model sees the denial and continues —
   **deny is a soft error, never fatal** (Claude Code's rejection semantics).
   A frontend wanting "deny and stop" composes deny with the cancel handle
   (ADR-0005 amendment, same change-set).

4. **No new `Event` variant in v1.** The denial is visible in the transcript
   (the `is_error` tool_result rides the existing `Event::Message`,
   `run.rs:107`) and in `Report.tool_calls`. An `Event::Approval` pair can be
   added later if trace consumers need decision *timing*; noted as an open
   extension, not done now.

## Consequences
- The core remains headless (ADR-0001 intact): nothing in the engine renders or
  waits on a terminal — it awaits a trait method that headless callers resolve
  instantly.
- ADR-0008's "Interactive approval prompts — out of scope" line gets a dated
  amendment pointing here (the *seam* is in scope; the *prompt* still is not).
- The TUI implements `Approver` with a oneshot + FIFO queue (grok's exact
  pattern) and any stickiness it wants.
- Tests: deny produces a paired `is_error` result and the run continues; batch
  with deny-then-allow keeps order and pairing; default approver preserves
  current behavior byte-for-byte (golden test on the exec integration suite).
