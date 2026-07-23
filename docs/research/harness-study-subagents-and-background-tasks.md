# Harness study — subagents, background tasks, and background bash

Source study of how the four studied CLIs implement **subagents** (a `Task`/
`spawn_agent`-style tool that runs a nested agent), **background tasks** (fire-
and-forget work the model is notified about), and **background bash** (long-
running shell commands), conducted 2026-07-22 against the `coding-cli-survey`
submodules. Citations are `harness: path:line`, relative to each submodule root
(`~/dev/coding-cli-survey/submodules/{claude-code,codex,grok-build,opencode}`).
Method: one deep source read per harness of the spawn tool(s), the context/loop
plumbing behind them, and the background-task registry + notification path; then
cross-comparison and a locode recommendation. This topic is flagged **Tier C**
in `tasks/todo.md` (subagents) with background bash at **Tier B**
(`tasks/todo.md:680-720`).

This document feeds a future ADR pair (proposed at the end) and does not itself
decide anything for locode.

---

## Scope

Three capabilities that all interact with the loop and the context window:

1. **Subagents** — a tool call that runs a *second* agent loop with its own
   context, tools, and (optionally) model, returning a distilled result to the
   parent. The question set: spawn tool schema; context isolation (fresh vs
   forked); tools/system-prompt selection; how the result returns and how much
   detail; agent type definitions (built-in vs file frontmatter); concurrency,
   caps, nesting depth; error/timeout/cancel propagation.

2. **Background tasks** — subagents or shell commands launched *asynchronously*:
   the tool returns immediately with a handle, the work runs off to the side,
   and the model is notified on completion (re-entering the loop as a later
   user-role message). The question set: launch/poll/kill tool schemas;
   lifecycle and buffering; a host-side task registry; cleanup.

3. **Background bash** — the shell-tool-specific slice of (2): a `run_in_background`
   / `is_background` flag plus output-retrieval and kill tools.

The through-line for locode: **where does this live relative to the one dispatch
door (ADR-0008), the cancellation seam (ADR-0018), the approval seam
(ADR-0017), and the streaming event protocol (ADR-0014)?**

---

## Per-harness findings — Subagents

### Claude Code — `Agent` tool (aka `Task`), hierarchical sidechain

**Spawn tool & schema.** The tool is named `Agent` (legacy alias `Task`;
`claude-code: src/tools/AgentTool/constants.ts:1-3`). The model-facing base
schema (`src/tools/AgentTool/AgentTool.tsx:82-88`):

```
description       string            "A short (3-5 word) description of the task"
prompt            string            "The task for the agent to perform"
subagent_type     string  optional  which specialized agent type
model             enum   optional   'sonnet'|'opus'|'haiku' override
run_in_background boolean optional  "run in the background … notified when it completes"
```

Gated extensions merge in multi-agent fields (`name`, `team_name`, `mode`) and
isolation (`isolation: 'worktree'|'remote'`, `cwd`) —
`AgentTool.tsx:91-102`. Fields are `.omit()`-stripped from the schema when their
feature is off so the model never sees a dead param (`AgentTool.tsx:110-125`).

**Context isolation.** Fresh by default. `runAgent()` builds
`initialMessages = [...contextMessages, ...promptMessages]`
(`src/tools/AgentTool/runAgent.ts:370-373`) where `contextMessages` is empty
unless a **fork** path supplies parent history (fork subagent feature —
`forkContextMessages`, `runAgent.ts:368-373`; the "fork yourself, omit
`subagent_type`" mode described in `src/tools/AgentTool/prompt.ts`
whenToForkSection). The subagent gets its **own system prompt** from the agent
definition (`getAgentSystemPrompt`, `runAgent.ts:911-943`), its **own tool
pool** (`resolveAgentTools`), and optionally its own model. Read-only agents
even drop the parent's CLAUDE.md hierarchy to save tokens
(`loadAgentsDir.ts:128-134`, comment "the main agent has full context and
interprets their output").

**Nested loop.** The subagent literally re-enters the same generator:
`for await (const message of query({ messages: initialMessages, … }))`
(`runAgent.ts:748-805`) — the identical `query()` loop the main thread runs.
So a subagent *is* a tool call that drives a nested `sample→dispatch→append`
loop.

**Result return — final text only.** `finalizeAgentTool()` extracts the **last
assistant message's `text` blocks** (falling back to the most recent assistant
message that has any text if the final turn was pure `tool_use`) —
`src/tools/AgentTool/agentToolUtils.ts:276-360`. Tool calls, intermediate
reasoning, and transcript noise are dropped; only the distilled report crosses
back. The Task tool prompt reinforces this: "it will return a single message
back to you. The result returned by the agent is not visible to the user"
(`prompt.ts`, Usage notes).

**Agent type definitions.** Built-in agents are code objects
(`BuiltInAgentDefinition`) — e.g. `general-purpose` (`tools: ['*']`, model
omitted → default subagent model; `built-in/generalPurposeAgent.ts:26-34`),
`Explore` (read-only, `model: haiku` for external / `inherit` for internal;
`built-in/exploreAgent.ts:65-78`), `Plan`, `verification` (both `model:
'inherit'`). User/project agents load from **`.claude/agents/*.md` frontmatter**
(`loadMarkdownFilesForSubdir('agents', cwd)`, `loadAgentsDir.ts:308`). The
frontmatter schema (`loadAgentsDir.ts:70-98`): `description`, `tools[]`,
`disallowedTools[]`, `prompt`, `model` (or `"inherit"`), `effort`,
`permissionMode`, `mcpServers[]`, `hooks`, `maxTurns`, `skills[]`, `memory`,
`background`, `isolation`. Precedence: user > project > managed > plugin >
built-in (`loadAgentsDir.ts:194-200`).

**Concurrency & nesting.** Parallel spawns are encouraged — "use a single
message with multiple tool uses" (`prompt.ts` concurrencyNote). Crucially,
**subagents cannot spawn subagents for external users**: the `Agent` tool is in
`ALL_AGENT_DISALLOWED_TOOLS` unless `USER_TYPE === 'ant'`
(`src/constants/tools.ts:36-46`, comment "enables nested agents"). So the tree
is one level deep in shipped builds. Async agents get a *restricted* tool
allowlist (`ASYNC_AGENT_ALLOWED_TOOLS`, `constants/tools.ts:54-71`).

**Error / cancel / partial.** `AbortError` re-throws for proper interruption
(`AgentTool.tsx:1128-1145`); a sync-agent error still tries to
`finalizeAgentTool` whatever messages exist so the parent sees partial progress
(`AgentTool.tsx:1230-1245`, "allows the parent agent to see partial progress
even after an error"). On any exit path a `finally` block kills the agent's
background bash tasks, clears its todos, releases fork context, cleans MCP
(`runAgent.ts:815-860`) — subagent teardown is thorough.

### Grok Build — `task` tool, hidden child sessions, depth-1 cap

**Spawn tool & schema.** The tool id is `task`
(`grok-build: crates/codegen/xai-grok-tools/src/implementations/grok_build/task/mod.rs:84`).
`TaskToolInput` (`crates/common/xai-tool-types/src/task.rs:13-110`) with
verbatim `#[schemars(description=…)]`:

```
prompt              String                required   full task prompt
description         String                required   3-5 words
subagent_type       String   default="general-purpose"   built-ins: general-purpose|explore|plan
run_in_background   bool     default=TRUE  returns immediately with subagent_id
capability_mode     enum?    read-only|read-write|execute|all
isolation           enum?    none(default)|worktree
resume_from         String?  continue a prior completed subagent's transcript
cwd                 String?  explicit working dir (mutually excl. with worktree)
model               String?  slug override, else inherit parent
task_id             (server-injected, #[schemars(skip)]) → becomes child session id
```

Note the **default `run_in_background = true`** (`task.rs:38-43`) — the opposite
of Claude Code (foreground default). Args are lenient-bool decoded but the
project's fidelity note (Type-strict tool args) is about *our* ports, not grok's.

**Context isolation.** Three modes (`InitialContextSource`,
`crates/codegen/xai-grok-shell/src/agent/subagent/mod.rs:44-57`):
- `New` — fresh session, no inherited history.
- `Forked` — parent history injected as a `<background_context>` chat-prefix
  ("harness-only chat-prefix fork"); the child's `forked_conversation` inherits
  a `prefix_len` (`agent/subagent/handle_request.rs:516,540-561,1028`).
- `Resumed` — inherits the source subagent's raw transcript, tool state, and
  model; system prompt is re-rendered (`mod.rs:48-56`, `resume_from` arg).

Child sessions **share the parent's hunk tracker, filesystem, terminal, and env**
so edits/bash/reads go through the same backends (`mod.rs:9-12`). Each still has
its own `SessionHandle` / `SessionThread` and cancel token (`SubagentTracker`,
`mod.rs:58-90`).

**Result return.** `SubagentCompletedOutput` (`task.rs:206-230`) carries
`output` (the answer text), plus run stats (`tool_calls`, `turns`,
`duration_ms`), `worktree_path`, and a **`resume_from_hint`** = the subagent_id
to continue it. `to_model_text()` renders the answer followed by a
`<subagent_meta>` stats line and a `<subagent_result>` resume footer
(`task.rs:270-310`). So grok returns final text **plus explicit resume
affordance** — richer than Claude Code's bare text.

**Nesting cap.** `MAX_SUBAGENT_DEPTH = 1` — "A top-level session is depth 0; the
first subagent is depth 1. **Subagents cannot spawn further subagents.**"
(`task/mod.rs:29-30`). Same one-level tree as Claude Code.

**Concurrency & blocking.** Background is the default; the model polls via a
multi-id output tool (see Background section). Foreground/blocking children exist
too (`block_waited`, `surface_completion`; `mod.rs:83-100`,
`SubagentQueryRequest{ block, timeout_ms }`, `task.rs:374-388`). A
`SubagentCoordinator` owns the active-subagent map on `MvpAgent`; spawn is a free
async fn `handle_subagent_request()` that never borrows `MvpAgent` (`mod.rs:1-12`).

**Error / cancel.** Per-subagent `CancellationToken`; background subagents are
excluded from `cancel_by_parent_prompt_id` "so the user can poll them"
(`task.rs:53-58`). Cancelled/killed/blocked-budget-exceeded are distinct result
states (`SubagentResult` flags, `task.rs:306-333`).

### Codex — `spawn_agent`, a **peer-agent mesh** with mailboxes

Codex is the odd one out: not a hierarchical "return a value" subagent but a
**collaborative multi-agent system** where agents are peer threads that message
each other. Two generations coexist (v1 legacy, v2 current);
`multi_agent_v2` is the live design.

**Spawn tool & schema.** `spawn_agent`
(`codex-rs/core/src/tools/handlers/multi_agents_spec.rs:96-137`, v2 properties at
`:616-655`):

```
task_name         string  required   "lowercase letters, digits, underscores"
message           string  required   initial plain-text task (.with_encrypted())
agent_type        string  optional   role name; available roles listed in desc
fork_turns        string  optional   "none" | "all" (default) | positive int (recent-N)
model             string  optional   override, else inherit parent
reasoning_effort  string  optional   override, else inherit
service_tier      string  optional
```

**Context isolation — `fork_turns`.** Unique dial: the spawned agent forks the
parent's rollout history — `"all"` passes full surrounding context, `"none"`
passes nothing, an integer forks only the most recent N turns
(`multi_agents_spec.rs:629-635`, `spawn.rs` `fork_mode()`,
`SpawnAgentForkMode::FullHistory`). Description warns: `fork_turns="none"` "may
cause the agent to lack the context it needs"; `"all"` gives full context
(`multi_agents_spec.rs:753-757`).

**Canonical task names & the mesh.** A spawned agent gets a **path name** —
"If your current task is `/root/task1` and you spawn_agent with task_name
`task_3` the agent will have canonical task name `/root/task1/task_3`"
(`multi_agents_spec.rs:743-746`). Agents address each other via
`send_message`/`send_input` (`multi_agents_spec.rs:143-205`), and — unlike the
other three — **"The spawned agent will have the same tools as you and the
ability to spawn its own subagents"** (`:747`). Depth is a *configurable* cap
(`turn.config.agent_max_depth`, enforced by `exceeds_thread_spawn_depth_limit`,
`multi_agents_v2/spawn.rs:5`, `multi_agents/spawn.rs:66-67`) — default 1 per
tests (`agent/registry_tests.rs:55-69`) but genuinely nestable.

**Result return — via mailbox, not the tool result.** `wait_agent` returns
`{ message: "Brief wait summary without the agent's final content", timed_out }`
(`multi_agents_spec.rs:506-521`). The agent's actual final answer is delivered as
a **`<subagent_notification>`** contextual user fragment
(`codex-rs/core/src/context/subagent_notification.rs:5-42`, JSON body
`{agent_path, status}`) and through the messaging channel — not inline in the
spawn/wait tool result. This is the async-mailbox model.

**Agent roles.** Roles resolve from built-in configs + user `config.toml`
`[agent_roles]` (`codex-rs/core/src/agent/role.rs:4-12,220-246`); each role can
pin `model` and `model_reasoning_effort` ("These settings cannot be changed",
`role.rs:272-276`). The spawn tool description lists available roles dynamically.

**Loop posture.** The spawn tool description is heavy on **when to delegate**:
"Do not spawn sub-agents unless the user or AGENTS.md/skill explicitly ask… Call
wait_agent very sparingly… While the subagent is running, do meaningful
non-overlapping work immediately" (`multi_agents_spec.rs:697-730`). Delegation is
async-first; blocking is discouraged.

### opencode — `task` tool, child sessions, experimental background

**Spawn tool & schema.** `task` (`opencode: packages/opencode/src/tool/task.ts`).
Effect-Schema parameters (`task.ts:43-64`):

```
description    string             3-5 words
prompt         string             the task
subagent_type  string             agent type
task_id        string  optional   resume a prior subagent session (same child session)
command        string  optional   the command that triggered this task
background      boolean optional   [flag-gated] async; notified on completion
```

`background` only appears when `OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS=true`
(`task.ts:96-101,391-398`) — otherwise the schema is the base struct without it
(`jsonSchema: … BaseParameters`).

**Context isolation.** A subagent is a **child `Session`** with `parentID` set to
the caller (`sessions.create({ parentID: ctx.sessionID, agent: next.name, … })`,
`task.ts:148-166`). Fresh context unless `task_id` resumes an existing child
session, which "continues with its previous messages and tool outputs"
(`task.txt` note 4; `task.ts:135-140`). Depth is capped by config
`subagent_depth ?? 1` — walk `parentID` chain, reject at limit
(`task.ts:104-113`). Child tool permissions are **derived and narrowed**:
`deriveSubagentSessionPermission` plus explicit denies of `todowrite` and the
`task` tool itself unless the agent opts in (`task.ts:141-158`) — so opencode
also blocks nesting by default.

**Result return.** Last text part of the child session:
`result.parts.findLast(item => item.type === "text")?.text ?? ""`
(`task.ts:203-207`), wrapped in a `<task id=… state=…><task_result>…` envelope
(`renderOutput`, `task.ts:66-82`). Final text only, same as Claude Code.

**Background lifecycle.** A `BackgroundJob` service (`src/background/job.ts`)
with `start/extend/wait/waitForPromotion/promote/cancel` (`job.ts:25-30`). A neat
twist: a **foreground** task can be *promoted* to background mid-flight —
`Effect.raceFirst(background.wait, background.waitForPromotion)` (`task.ts:296-305`).
On background completion, `inject()` prompts the **parent session** with a
`synthetic: true` user text part carrying the `<task_result>` block
(`task.ts:210-236`) — re-entering the parent loop as a later message.

---

## Per-harness findings — Background tasks & background bash

### Claude Code — unified task registry + `<task-notification>` re-entry

**Background bash.** `Bash` tool schema adds
`run_in_background: boolean` — "Set to true to run this command in the
background. Use Read to read the output later"
(`claude-code: src/tools/BashTool/BashTool.tsx:241`). Output fields include
`backgroundTaskId`, and there is also **auto-backgrounding**: a blocking command
that exceeds the assistant-mode budget is moved to the background with an ID
(`BashTool.tsx:285-288,608-612`). Blocking `sleep`/poll patterns are actively
refused with guidance to use `run_in_background` or the `Monitor` tool
(`BashTool.tsx:525-531`).

**Unified registry.** `AppState.tasks` is a process-global map keyed by task id;
`TaskState` is a union over **shell, agent, remote agent, in-process teammate,
workflow, monitor-MCP, dream** tasks (`src/tasks/types.ts:12-38`). Same registry,
one notification path — background bash and background subagents are the same
mechanism.

**Retrieval & kill tools.** `TaskOutputTool` (input `{task_id, block, timeout}`,
`src/tools/TaskOutputTool/TaskOutputTool.tsx:30-33`) is **deprecated in favor of
`Read` on the output-file path** — background tasks return an `output_file`, and
the completion notification carries the same path (`TaskOutputTool.tsx:173-177`).
`TaskStopTool` kills by `task_id` (`src/tools/TaskStopTool/TaskStopTool.ts:15-32`).

**Completion → re-entry.** On completion, `enqueueShellNotification` /
`enqueueAgentNotification` push a `<task-notification>` message onto a
process-global pending-notification queue with a `mode: 'task-notification'` and
an `agentId` scope (`src/tasks/LocalShellTask/LocalShellTask.tsx:105-176`). The
message is an XML block: `<task_id>`, `<output_file>`, `<status>`, `<summary>`
(`LocalShellTask.tsx:161-170`). The **loop drains it at turn boundaries**,
converting queued commands to attachment (user-role) messages, scoped so the main
thread drains `agentId === undefined` and subagents drain only their own
(`src/query.ts:1560-1633`). Completion literally re-enters the loop as a later
user message. A stall detector notices interactive-prompt blocks and notifies
"likely blocked on an interactive prompt… kill and re-run with piped input"
(`LocalShellTask.tsx:80-96`).

**Cleanup.** Subagent teardown kills its background bash tasks
(`killShellTasksForAgent`, `runAgent.ts:843-847`); session exit reaps them so a
`run_in_background` loop doesn't become a PPID=1 zombie (same comment).

### Grok Build — host task registry + auto-wake notification bridge

**Background bash.** `BashToolInput.is_background` — "Set to true for long-running
commands… Returns a task_id immediately while the command keeps running; you are
notified on completion, so do not poll or sleep-wait for it"
(`grok-build: crates/codegen/xai-grok-tools/src/implementations/grok_build/bash/mod.rs:281-288`).
`timeout: 0` in background mode disables the wrapper timeout entirely (`mod.rs:260-271`).

**Retrieval & kill tools.** `get_task_output` (`TaskOutputToolInput{ task_ids:
Vec<String>, timeout_ms: Option<u64> }`, `task.rs:315-333`) — **multi-id**: one
or more ids, positive `timeout_ms` waits until all complete, omit/0 polls;
`MAX_MULTI_WAIT_IDS = 20` (`task.rs:311`). Output is a per-task `TaskOutputResult`
with status/exit_code/started/ended/duration/output/output_file/truncated/
raw_output_bytes (`task.rs:389-420`) or a `MultiTaskOutputResult`
(`task.rs:433-437`). `kill_task` takes a single `task_id`
(`crates/codegen/xai-grok-tools/src/implementations/grok_build/kill_task/mod.rs`;
not-found errors list known ids, `mod.rs:56-63`). A legacy `wait_tasks` alias is
kept "for compatibility" and points at `get_command_or_subagent_output`
(`task.rs:1036,1472-1479`).

**Unified across bash + subagents.** `get_task_output` takes ids "from
background=true commands or background=true subagents"
(`task.rs:1443,1462`) — one retrieval tool spans both, exactly like Claude Code's
registry.

**Notification bridge.** `notification_bridge.rs` correlates `tool_call_id` ↔
`task_id`, populates the tasks panel, and does **auto-wake**: when a background
task completes and no blocking waiter consumed it, it injects a synthetic prompt
(`format!("task-completed-{task_id}")` / `bash-completed-{task_id}`) so the model
re-enters (`crates/codegen/xai-grok-shell/src/tools/notification_bridge.rs:256-513`).
`auto_wake_delivered` dedups so a blocking-return or `kill_task` result doesn't
double-notify (`notification_bridge.rs:335-360`). A dedicated `monitor` tool
streams events (distinct from one-shot background bash).

### Codex — no fire-and-forget bash; **cooperative-yield exec sessions**

Codex has **no `run_in_background` boolean and no task registry** for shell. Its
`exec_command` runs a command in a **PTY and yields**: it waits up to
`yield_time_ms` (default 10000 ms, range 250–30000) and, if the command is still
running, **returns a `session_id` instead of blocking**
(`codex-rs/core/src/tools/handlers/shell_spec.rs:15-113`, description "Runs a
command in a PTY, returning output *or a session ID for ongoing interaction*").
The model then drives the live process with **`write_stdin`** — non-empty writes
send input, an **empty write is a background poll** that returns recent output
(`shell_spec.rs:112-155`; `unified_exec/write_stdin.rs:22-27,85-122`, comment
"Empty stdin is a background poll"). `max_output_tokens` bounds output per call
(default 10000). So "background" in codex is **model-driven polling of a
persistent, still-attached PTY session**, not a detached task the host notifies
about. Long-running work overlaps with model turns only insofar as the model
chooses to poll it.

Background *subagents* (the mesh above) do run asynchronously and notify via
`<subagent_notification>`, but shell itself is cooperative-yield.

### opencode — **no background bash**; background = subagent-only, experimental

The `shell` tool is **foreground with a hard timeout** — no background flag; the
run races the child against a `timeout` sleep and terminates on expiry with a
"retry with a larger timeout" message (`opencode: packages/opencode/src/tool/shell.ts:540-564,615-618`).
Long-running background execution exists **only** for subagent `task`s, behind
the experimental flag, via the `BackgroundJob` service (see opencode subagents
above). There is no `get_task_output`/`kill_task` model-facing tool pair; the
background result is *pushed* into the parent session via a synthetic prompt
(`task.ts:210-236`) rather than *pulled*.

---

## Comparison

### Subagents

| Dimension | Claude Code | Grok Build | Codex (v2) | opencode |
|---|---|---|---|---|
| Spawn tool | `Agent` (alias `Task`) | `task` | `spawn_agent` | `task` |
| Required args | description, prompt | prompt, description | task_name, message | description, prompt, subagent_type |
| Type selector | `subagent_type` | `subagent_type` (def general-purpose) | `agent_type` (role) | `subagent_type` |
| Model override | `sonnet\|opus\|haiku` | `model` slug | `model` + `reasoning_effort` | agent def only |
| Context default | **fresh** (fork opt-in) | **forked/background_ctx** or new; bg default | fork_turns=`all` default | **fresh** (resume via task_id) |
| Context dial | fork (omit subagent_type) | New/Forked/Resumed | `fork_turns` none/all/N | `task_id` resume |
| Result to parent | **last assistant text** | text + stats + resume hint | **via mailbox**, not tool result | last text part, XML-wrapped |
| Nesting | blocked (ext); ant-only | **depth 1 hard** | configurable `agent_max_depth` | blocked (`subagent_depth??1`) |
| Peer messaging | teammates (swarm) | — | **send_message/send_input mesh** | — |
| Parallel spawn | encouraged | bg default → parallel | encouraged, async-first | encouraged |
| Type defs | built-in objs + `.claude/agents/*.md` | built-in + user types | built-in + `config.toml [agent_roles]` | agent registry |
| Isolation | worktree/remote | worktree/none | environments | derived permissions |

### Background tasks / background bash

| Dimension | Claude Code | Grok Build | Codex | opencode |
|---|---|---|---|---|
| Background bash flag | `run_in_background` + **auto-bg** | `is_background` (`timeout:0` = no cap) | **none** (exec yields session_id) | **none** (timeout only) |
| Model | detached task + notify | detached task + notify | cooperative-yield PTY poll | subagent-only bg |
| Registry | `AppState.tasks` union (shell+agent+…) | host task registry (bash+subagent) | per-session exec sessions | `BackgroundJob` service |
| Retrieve output | `Read` file path (TaskOutput deprecated) | `get_task_output` (multi-id, ≤20) | `write_stdin` empty poll | pushed, no pull tool |
| Kill | `TaskStop{task_id}` | `kill_task{task_id}` | (session ends / interrupt) | `background.cancel` |
| Completion signal | `<task-notification>` user msg | auto-wake synthetic prompt | `<subagent_notification>` / final in mailbox | synthetic user prompt inject |
| Re-entry unit | drained at turn boundary, agent-scoped | notification bridge injects prompt | mailbox fragment | forked prompt into parent |
| Cleanup | kill on agent/session exit | reaped; dedup notify | session-scoped | scope-tied |

**Two axes emerge.** (1) *Detached-and-notify* (Claude Code, Grok) vs
*attached-and-poll* (Codex) vs *push-only-subagent* (opencode). (2) *Unified
registry spanning bash and subagents* (Claude Code, Grok) vs *separate
mechanisms* (Codex, opencode). Claude Code and Grok are near-convergent; they are
the model to copy for a task registry.

---

## Pros / cons & best practice

### Context-holding vs fresh subagents
- **Fresh (Claude Code/opencode default)** — the point of a subagent is to keep
  the parent context clean: burn tool noise in the child, return only a distilled
  report. Best for *research/search fan-out* where intermediate reads are dead
  weight. Cost: you must brief the child fully ("smart colleague who just walked
  into the room", `claude-code: prompt.ts`) — terse prompts yield shallow work.
- **Forked (Grok `Forked`, Codex `fork_turns=all`, Claude fork mode)** — child
  inherits parent context and (Grok/Claude) **shares the prompt cache**, so a
  fork is cheaper than a fresh subagent and needs only a *directive*, not a
  briefing. Best for *implementation* work continuous with the current thread.
  Cost: pulls the parent's whole context into the child; don't set a different
  model on a fork (breaks cache reuse; `claude-code: prompt.ts` whenToFork).
- **Best practice:** offer *both*, defaulting to fresh, with an explicit
  context-inheritance dial. Codex's `fork_turns` (none/all/N) is the cleanest
  single knob.

### Fan-out patterns
- All four encourage **parallel independent subtasks in one message**. The
  discipline that matters (Codex spells it out best,
  `multi_agents_spec.rs:718-730`): **disjoint write sets** for code-edit fan-out,
  don't delegate the *immediate blocking* step, do non-overlapping local work
  while children run, don't redo delegated work.
- **Don't-peek / don't-race** (Claude Code fork prompt): after launching an async
  child, the parent knows nothing; never fabricate its result; reading its
  transcript mid-flight defeats the isolation.

### Detach-and-notify vs poll
- **Detach-and-notify (Claude Code, Grok)** keeps the conversation responsive and
  avoids `sleep`-loops (both actively *refuse* blocking sleeps). Completion
  re-enters as a later user message — clean for a headless JSONL trace. Requires
  a host task registry + a notification queue drained at turn boundaries.
- **Cooperative-yield (Codex)** is simpler infra (no registry, no async
  notification) and keeps the model in control, but it burns model turns on
  polling and couples "background" to the model's diligence.
- **Best practice for a headless engine:** detach-and-notify. The whole value is
  that the loop doesn't block and the trace stays linear.

### Error / cancel semantics
- **Partial results on error** (Claude Code finalizes whatever messages exist;
  Grok has explicit partial/budget-exceeded states) — a subagent failure should
  hand the parent *something*, not just an opaque error.
- **Per-child cancellation token**, and **background children excluded from
  bulk parent-prompt cancel** (Grok, `task.rs:53-58`) so a user Esc on the parent
  turn doesn't nuke independent background work the user still wants.
- **Reap on exit** — kill a subagent's background bash when it ends; kill all
  tasks on session teardown (Claude Code) to avoid zombies.
- **Dedup notifications** (Grok `auto_wake_delivered`) so a blocking-wait return
  and the async notification don't both fire.

### Nesting
- Three of four **cap nesting at 1** (Claude Code blocks the Agent tool in
  subagents for external users; Grok `MAX_SUBAGENT_DEPTH=1`; opencode
  `subagent_depth??1`). Only Codex's peer-mesh genuinely nests, and it's
  config-gated. **Best practice for v0: cap at depth 1.** Deep trees explode
  cost and are hard to reason about.

### When to use which
- **Subagent (fresh)** — bounded research/search you don't want in context.
- **Fork** — implementation continuous with the current thread; cache-friendly.
- **Background bash** — dev servers, long builds, log tails; anything where the
  model should keep working. Not for quick commands (overhead).
- **Foreground everything else** — when you need the result before the next step,
  don't background it and immediately wait (all four warn against reflexive wait).

---

## Recommendation for locode

**Framing.** Both features are **loop-and-host concerns on the shared engine**,
not per-pack behavior — consistent with the fidelity boundary (memory: mimicry
stops at tools+prompts+preamble; loop-adjacent behavior stays on the shared
engine). A ported pack mimics the **tool surface** (names, arg schemas,
descriptions, defaults); the **engine** provides the one mechanism behind them.
This mirrors how all four harnesses actually build it: one registry / one nested
loop, many tool skins.

### B. Background bash — do this first (Tier B, smaller, already scaffolded)

`tasks/todo.md:562-563,720` already reserves the seam:
`Host::exec_background` + a task registry, `is_background`, a `<task-id>`
envelope, `get_task_output`/`kill_task`. Concrete shape:

1. **Host owns the registry.** Add a `TaskRegistry` in `locode-host` keyed by a
   `TaskId`, storing handle + status + an append-only output buffer written to a
   file under the workspace scratch dir (so retrieval is a normal `read_file`,
   à la Claude Code's "Read the output_file"). `Host::exec_background(cmd) ->
   TaskId`. This keeps the **one dispatch door** intact — background exec is
   still a tool call routed through `dispatch`; only the *waiting* is detached.
2. **Tool surface, per pack.** `run_terminal_cmd`/`Bash` gains the pack's real
   flag (`is_background` for grok, `run_in_background` for claude). Add
   `get_task_output` (grok: multi-id + `timeout_ms`; claude: prefer `read_file`
   on the path) and `kill_task`. Faithfulness: grok defaults, claude defaults —
   port each verbatim.
3. **Completion re-entry via the event protocol.** Emit a new
   `Event::TaskNotification { task_id, status, output_path, summary }` in
   `locode-protocol` (extends ADR-0014's `#[non_exhaustive]` enum), and inject a
   **synthetic user-role `Message`** at the next turn boundary carrying a
   `<task-notification>` block — exactly Claude Code's `query.ts:1560-1633` drain
   and Grok's notification bridge. The loop (ADR-0005 sample→dispatch→append)
   gets one new step: *before re-sampling, drain completed background tasks into
   history.* This is additive, not a rewrite.
4. **Cancellation (ADR-0018).** Each task holds a child token derived from the
   session token; session cancel reaps all tasks; a per-task `kill_task` cancels
   one. **Background tasks are excluded from the turn-level cancel** (copy Grok)
   so Esc on a turn doesn't kill independent background work — call this out as a
   deliberate ADR-0018 amendment.
5. **Cleanup.** Reap on session teardown (headless run end / SIGTERM) so no
   zombie children — Claude Code's explicit lesson.

### A. Subagents — Tier C, larger; build on (B)'s registry

1. **A subagent is a tool call that runs a nested engine loop.** `run_agent`
   (our `task`/`Agent`) constructs a child `Session` with its own
   context/tools/model and drives the *same* `sample→dispatch→append` loop
   (ADR-0005) — exactly Claude Code's `query()` recursion and opencode's child
   `Session`. No second loop implementation (Boundaries: "never introduce a
   second, throwaway loop").
2. **Return final text only** to the parent (Claude Code/opencode), optionally
   with a `<subagent_meta>` stats line + resume hint (Grok). Keep the transcript
   valid: the subagent tool_use pairs with exactly one tool_result carrying that
   text (ADR-0004).
3. **Context dial.** Default **fresh**; offer a Codex-style `fork_turns`
   (none/all/N) or Grok's New/Forked/Resumed. v0 can ship fresh-only and reserve
   the fork seam.
4. **Cap nesting at depth 1** (three of four harnesses) — the child's tool pool
   excludes the spawn tool. Cheap safety; revisit later.
5. **Agent type definitions.** Built-in types as code (general-purpose, explore,
   plan) + a `.locode/agents/*.md` (or `[agent_roles]` in config) loader with
   frontmatter `{description, tools, model, prompt, effort, permissionMode,
   maxTurns}`. This is a *shared-engine* capability; packs pick names/descriptions.
6. **Background subagents** reuse (B)'s registry + notification path — a subagent
   with `run_in_background` is just another `TaskState` variant, unifying
   bash+subagent tasks under one registry (Claude Code `tasks/types.ts`, Grok
   `get_task_output` spanning both). Grok defaults subagents to background; Claude
   to foreground — pack-level defaults.
7. **Approval (ADR-0017)** flows unchanged: the subagent's *inner* tool calls hit
   the same dispatch door and the same approval seam; spawning itself is
   auto-allowed (Claude Code `checkPermissions` returns allow;
   `AgentTool.tsx:isReadOnly()=true` delegating to underlying tools).

### Proposed ADRs
- **ADR-00XX: Background tasks & the host task registry** (Tier B). Registry in
  `locode-host`; `Event::TaskNotification`; turn-boundary drain step in the loop;
  `is_background`/`run_in_background` per pack; `get_task_output`/`kill_task`;
  cancel exclusion. *Small ADR, then mostly-autonomous* per the STATUS note.
- **ADR-00YY: Subagents (nested engine loop)** (Tier C). Nested `run_agent` on
  the shared loop; fresh-default context with a reserved fork dial; depth-1 cap;
  agent-type definitions (built-in + file frontmatter); final-text return;
  background subagents reuse the task registry. Reference ADR-0005/0008/0012/
  0014/0017/0018.

### Sequencing / complexity
Background bash first (self-contained, seam reserved, unblocks the grok/claude
pack audit criteria 9–10 in `tasks/todo.md:562`). Subagents second, reusing the
registry and notification path so the incremental cost is the nested-loop wiring
+ agent-type loader, not a new async subsystem. Both are **additive extension
points** to ADR-0005's loop, consistent with "reserved slots, not rewrites."

---

## Open questions

1. **Fork/cache in a headless engine.** Grok/Claude make forks cheap via prompt
   cache reuse; our provider trait (ADR-0007) doesn't model cache breakpoints. Is
   a `fork_turns`-style inherited-context subagent worth it in v0, or ship
   fresh-only and defer forks until caching is modeled? (Leaning: fresh-only v0.)
2. **Notification injection vs the JSONL trace (ADR-0014).** A `<task-notification>`
   re-entering as a synthetic user message must appear in the stream as a
   `message` event for replayability. Confirm it composes with `init/message/
   result` ordering and doesn't confuse the "every tool_use → one tool_result"
   invariant (it won't — it's a fresh user turn, not a tool_result).
3. **Turn-boundary drain vs strict serial dispatch (ADR-0005).** ADR-0005 is
   serial-first. Draining completed tasks *between* turns is compatible, but if we
   ever allow a blocking `get_task_output` mid-turn, that's a dispatch-time await
   on host state — is that still "one dispatch door"? (Yes: it's a normal tool
   awaiting host state, like the shell already does under ADR-0018.)
4. **Codex-style cooperative-yield exec** — do we want it *at all*, or is
   detach-and-notify strictly better for headless? Codex is the only harness
   without a task registry; adopting only detach-and-notify means the `codex` pack
   can't perfectly mimic `write_stdin` polling. Fidelity vs mechanism tension —
   flag for the pack audit. (Codex pack may need its own exec-session shim, or we
   accept an approximation and note it, per ADR-0012's "P0 behavior, P1 exact.")
5. **Nesting depth** — hard-cap at 1 for all packs, or make it a per-pack config
   to let the `codex` pack mimic nestable agents? (Leaning: cap 1 in v0, reserve
   a config seam.)
6. **Peer messaging (Codex `send_message`/mesh)** — out of scope for v0? It's a
   whole coordination layer only Codex has; the other three are hierarchical
   return-a-value. Recommend deferring; note it as a Codex-pack fidelity gap.
7. **Approval for spawning** — auto-allow (all four effectively do) or route the
   spawn itself through ADR-0017's seam in the TUI? Inner tool calls are already
   gated; gating the spawn too may be redundant friction.
