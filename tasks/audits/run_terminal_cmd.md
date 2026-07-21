# run_terminal_cmd — fidelity audit vs Grok Build

Paths used below:

- **OURS** = `/Users/luoliangchen/dev/locode-core/crates/locode-packs/src/grok/terminal.rs` (registration: `crates/locode-packs/src/grok/mod.rs`; host seam: `crates/locode-host/src/shell.rs`, `crates/locode-host/src/lib.rs`)
- **GB** = `~/dev/coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/`; unless prefixed, `bash/mod.rs` = `GB/src/implementations/grok_build/bash/mod.rs`, `terminal.rs` = `GB/src/computer/local/terminal.rs`, `output.rs` = `GB/src/types/output.rs`, `truncate.rs` = `GB/src/util/truncate.rs`.

## Verdict

DRIFT (3 schema issues, 11 behavior issues) — foreground-only skeleton is directionally right (name, `exit:` header, 120s/300s timeouts, group-kill), but the schema drops `is_background` and the `timeout` wire description, and nearly all of Grok Build's surrounding behavior (truncation shape, output file, shell detection, background mode, `&`/pkill guardrails, prompt post-processing) is missing or different.

## Schema comparison

Tool name: both register as `run_terminal_cmd` (GB `bash/mod.rs:1581`; ours `mod.rs:36`). GB required fields on the wire: `command`, `description` (schemars derive; `timeout` is `Option`, `is_background` has `#[serde(default)]` → optional; missing `description` fails deserialization, test `bash/mod.rs:2292`). Ours: same two required (`terminal.rs:30-43`).

| Field (wire name) | GB type / default / description (verbatim) | GB cite | Ours (verbatim) | Our cite | Status |
|---|---|---|---|---|---|
| `command` | `String`, required. Unix: **"The bash command to run."** (non-unix build: **"The command to run."**) | `bash/mod.rs:252-254` | `String`, required. **"The bash command to run."** | `terminal.rs:31-32` | **MATCH** (unix form) |
| `timeout` | `Option<u64>`, default absent; schema advertises integer but deserializer is **lenient** (accepts `"120000"` string, `null`; `bash/mod.rs:263-271`, test `2232-2253`). Struct-level attr: **"Optional timeout in milliseconds (max 300000). Default: 120000 (2 minutes). `timeout: 0` in background mode disables the wrapper timeout entirely; the task runs until it exits or is killed via the kill task tool."** (`bash/mod.rs:260-262`). **Exported wire description is rewritten** at definition time (default params): **"Optional timeout in milliseconds (max 300000). Default: 120000. `timeout: 0` in background mode disables the wrapper timeout entirely; the task runs until it exits or is killed via the kill task tool."** — note it drops "(2 minutes)"; a JSON-Schema `maximum` is added only when `max_timeout_secs` is configured | `bash/mod.rs:1351-1374` | `Option<u64>`, `#[serde(default)]`; strict u64 (a `"120000"` string is rejected — no lenient deserializer). **"Optional timeout in milliseconds (max 300000). Default: 120000 (2 minutes)."** | `terminal.rs:33-37` | **DESCRIPTION-DRIFT** (ours keeps "(2 minutes)" — the raw struct attr, not the exported wire form — and drops the entire `timeout: 0` background sentence) + strict-type behavior gap |
| `description` | `String`, required. **"One sentence explanation as to why this command needs to be run and how it contributes to the goal."** Not used for execution (UX/notifications only) | `bash/mod.rs:273-277` | `String`, required, `#[allow(dead_code)]`. **"One sentence explanation as to why this command needs to be run and how it contributes to the goal."** | `terminal.rs:38-42` | **MATCH** |
| `is_background` | `bool`, `#[serde(default)]` (→ optional in schema, advertised `boolean`), **lenient** deserializer (accepts `true`/`"true"`/`"yes"`/`"1"`/`1`; test `bash/mod.rs:2256-2293`). **"Set to true for long-running commands that should run in the background (e.g., dev servers, long builds). Returns a task_id immediately while the command keeps running in the background; you are notified on completion, so do not poll or sleep-wait for it."** Schema visibility: present whenever `enabled_background` (default `true`, `bash/mod.rs:160-161,213`); removed from `properties`/`required` only when backgrounding is disabled (`bash/mod.rs:1348-1350,1377-1381`) | `bash/mod.rs:279-288` | — | `terminal.rs:28-43` (absent; header comment "`is_background` dropped in v0" at `terminal.rs:28`) | **MISSING** (known/documented drop) |

## Tool description comparison

**DRIFT.**

GB's model-facing description is a rendered template. Verbatim template (background-enabled variant, the default; `bash/mod.rs:1421-1437`, selected at `bash/mod.rs:1413-1419`):

```
Run a ${%- if is_windows %} shell command${%- else %} bash command${%- endif %} and return its output.

Usage notes:
  - You can specify an optional timeout in milliseconds (up to ${{ max_timeout_ms | default(300000) }}ms). ${%- if auto_background_on_timeout %} If not specified, commands exceeding the default timeout will be automatically backgrounded instead of killed. You will receive a task_id to check output later.${%- else %} If not specified, commands will timeout after ${{ default_timeout_ms | default(120000) }}ms.${%- endif %}
  - Timeout enforcement: when the timeout fires, the wrapper${%- if is_windows %} terminates the child's Job Object, killing every descendant process immediately (no graceful-termination grace period).${%- else %} kills the child process group (SIGTERM, escalated to SIGKILL after a ~1s grace period). Descendants that did not detach via `setsid` / `nohup` will also be killed.${%- endif %} `timeout: 0` in `${%- if params is defined and params.execute is defined and params.execute.is_background %}${{ params.execute.is_background }}${%- else %}background${%- endif %}: true` mode disables the wrapper timeout entirely; the child's lifetime is owned by the model via ${{ tools.by_kind.kill_task_action }}.
  - If the output exceeds {max_output_bytes} characters, output will be truncated before being returned to you.
  - You can use the ${{ params.execute.is_background }} parameter to run the command in the background (e.g., dev servers, long builds): it returns a task_id immediately and keeps running in the background. You are notified on completion, so do not poll or sleep-wait for it.${%- if has_unix_utilities %} You do not need to use '&' at the end of the command when using this parameter.${%- endif %}
${%- if shell_uses_semicolon %}
  - '&&' is not supported in this shell; chain sequential commands with ';'.
${%- endif %}
${%- if not has_unix_utilities %}
  - The Unix utilities `grep`, `head`, `tail`, `sed`, `awk`, and `find` are NOT available in this shell. Use the dedicated tools instead.
${%- endif %}
```

Placeholder resolution: `{max_output_bytes}` → 20000 by default (`GB/src/types/context.rs:76-91` + `GB/src/lib.rs:11`); `${{ params.execute.is_background }}` → the client-facing name of the `is_background` param; `${{ tools.by_kind.kill_task_action }}` → the kill tool's client name (`kill_task`, `GB/src/implementations/grok_build/kill_task/mod.rs:158`); numbers from `effective_max_timeout_ms`/`effective_default_timeout_ms` (`bash/mod.rs:1397-1404`). Rendered on a default unix session this yields (my reconstruction from the template — the fragments are verbatim, the assembly is rendered): "Run a bash command and return its output." + the four usage notes with 300000ms / 120000ms / ~1s SIGTERM→SIGKILL / 20000 characters / `is_background` guidance. A background-disabled variant exists at `bash/mod.rs:1440-1453`.

Ours (`terminal.rs:88-90`), verbatim:

```
Run a bash command in the workspace shell and return its combined output and exit code.
```

Single sentence, none of GB's usage notes (timeout numbers, kill semantics, truncation threshold, background guidance). DRIFT.

## Behavior comparison

- **Shell invocation.** GB: detects the user's shell — bash *or zsh* (`ShellKind::detect`, `GB/src/computer/local/shell_state.rs:184-205`) — and spawns `<shell> -c <command>` (non-login; zsh additionally gets `-o nonomatch`, `terminal.rs:2724-2730`), stdin null, stdout/stderr piped (`terminal.rs:2732-2734`). Env: request env + shell env overrides + pager env (`terminal.rs:2741-2746`), then the user's **login-shell PATH captured once** via a separate `<shell> -lc 'source ~/.rc; printf PATH'` probe (5s timeout, `terminal.rs:2623-2693`) layered last (`terminal.rs:2748-2755`); agent marker env wins over all (`terminal.rs:2756-2757`); child detached from the controlling tty into its own session/process group (`terminal.rs:2758-2761`). Ours: fixed `shell_program` (default `"bash"`, `lib.rs:87`) with **`-lc` (login shell) by default** (`shell.rs:159-162`, `lib.rs:88`), stdin null + pipes (`shell.rs:164-166`), only `req.env` extras (empty from the tool: `terminal.rs:105`), `process_group(0)` (`shell.rs:172-173`). Net drift: `-lc` vs `-c`+PATH-probe (similar intent, different mechanism — a login shell re-sources profiles per call and can pollute output), no zsh detection, no pager env, no tty detach.
- **cwd handling.** GB resolves cwd from the session `Cwd` resource (`bash/mod.rs:1788`) and passes it as `working_directory` (`terminal.rs:2731`); the backend is a *persistent shell session* — shell state (cwd via state dump scripts) carries across calls (`bash/mod.rs:3`, `terminal.rs:1998`, `shell_state.rs:73,128`; the backgrounded prompt even reports "On the next terminal tool call, the directory of the shell will be …", `bash/mod.rs:429-430`). Ours: fresh process per call at `ctx.cwd` (`terminal.rs:101-104`); no state persistence (exact GB state-carryover mechanics beyond cwd: UNVERIFIED — I did not trace the full shell_state re-injection path).
- **Timeout default/max.** GB: default 120s (`DEFAULT_TIMEOUT`, `bash/mod.rs:960`; `effective_default_timeout_ms` `bash/mod.rs:1216-1226`), model ceiling 300000ms by default (`DEFAULT_MAX_TIMEOUT_MS`, `bash/mod.rs:457`; production grok-build opts up to 10h via config, `bash/mod.rs:450-457`), absolute clamp 10h (`bash/mod.rs:458`); additionally a non-backgroundable foreground command is clamped to `MAX_FOREGROUND_BLOCK` = 5 min (env-overridable, `bash/mod.rs:968-976,2024-2028`). Enforcement: SIGTERM to the process group, ~1s grace, then SIGKILL (`terminal.rs:45-46,1643-1650`, description `bash/mod.rs:1429`). Ours: default 120_000ms, cap 300_000ms, `min`-clamped (`terminal.rs:14-15,97-100`); host kills group SIGTERM → **2s** grace → SIGKILL (`shell.rs:205-234`, `kill_grace` default `lib.rs:58`); host `max_timeout` 10min never binds (`shell.rs:70-73`, `lib.rs:56`). Match on numbers/kill shape; grace differs 1s vs 2s.
- **Output caps + truncation marker.** GB: cap 20,000 **chars** (`DEFAULT_TOOL_OUTPUT_CHARS`, `GB/src/lib.rs:11`, applied `bash/mod.rs:1823-1836`), **front-and-back**: first 10k chars frozen, last 10k kept (`maybe_truncate`, `terminal.rs:346-380`), rejoined with the exact in-body separator `"\n\n... (output truncated) ...\n\n"` (front `trim_end`ed, back `trim_start`ed, `terminal.rs:310-317`); the prompt header gains the exact annotation `" [truncated: showing first/last {shown} of {total} - full output at: {output_file}]"` with sizes via `format_bytes` (`"12.3KB"`-style, `truncate.rs:200-208`) (`bash/mod.rs:385-394`). Full untruncated output is streamed to `{session_folder}/terminal/{tool_call_id}.log` (`bash/mod.rs:1916-1918`; file caps: 5 GiB during run, retained file truncated to 64 MiB, `terminal.rs:66-82,382-392`). Ours: cap 30,000 **bytes per stream** (`lib.rs:57`), **tail-only** retention (oldest dropped, `shell.rs:180-201`), marker is the exact header suffix `" [output truncated]"` (`terminal.rs:69-74`); no output file, no sizes. Every element drifts: unit, amount, strategy, marker text, file fallback.
- **stdout/stderr merging.** GB: both pipes drained non-blocking each ~100ms tick into one interleaved buffer, stdout first within a tick (`terminal.rs:1528-1589`,`1599-1601`) → `combined_output` (`terminal.rs:309-320`). Ours: streams captured separately and concatenated **whole-stdout then whole-stderr** with `\n` (`shell.rs:141-145,241-249`; used `terminal.rs:119`). Ordering semantics differ for interleaved output.
- **Prompt post-processing.** GB strips ANSI escapes and soft-wraps lines at 2000 chars (`output.rs:453-458`, `truncate.rs:4`; re-formatted via `format_default_prompt`, `bash/mod.rs:2159,2173`). Ours: raw text, no stripping/wrapping (`terminal.rs:74`).
- **Exit-code presentation.** GB header: `exit: {code}` normally; when killed, `exit: killed ({reason})` where reason ∈ `timeout` / `max_runtime` / `cancelled` / `killed` / `signal N` (`KillReason`, `bash/mod.rs:341-383`, header `bash/mod.rs:433-442`); a non-synthetic signal string instead annotates `" [signal={s}]"` with `exit: -1` (`bash/mod.rs:395-403`); struct `exit_code` sentinel is `-1` via `unwrap_or(-1)` (`bash/mod.rs:2161`). Ours: `exit: {code}`, `exit: killed (timeout)`, or bare `exit: killed` (`terminal.rs:60-74`); exit sentinel `-1` (`terminal.rs:116`). Shape matches; GB's reason granularity (`cancelled`, `signal N`, `max_runtime`) and `[signal=…]` annotation are missing.
- **Background mode (GB full semantics; ours: absent).**
  - *Spawn/track:* `is_background: true` → `backend.run_background` registers the process in the terminal actor's task table and returns `BackgroundHandle { task_id, output_file, pid }` immediately (`bash/mod.rs:1920-1968`; `PYTHONUNBUFFERED=1` injected, `bash/mod.rs:1922-1926`). Timeout omitted/`0` → unbounded (`Duration::MAX`), positive → clamped only by the 10h absolute limit (`bash/mod.rs:1242-1270`); the actor's `BACKGROUND_MAX_RUNTIME` (10h) is the backstop kill (`terminal.rs:48-50,1209-1232`).
  - *Model-visible return:* `BashToolOutput::Background(BackgroundTaskStarted { task_id, task_type: "bash", output_file, status: "running", command, summary: "Background task {id} started", retrieval_hint, pid })` (`bash/mod.rs:1998-2011`), rendered as an XML envelope `<task-id>…</task-id>\n<task-type>…</task-type>\n<output-file>…</output-file>\n<status>…</status>\n<summary>…</summary>\n{retrieval_hint}` (`output.rs:775-793`).
  - *Output/retrieval:* live output streams to the task's log file (`terminal.rs:1585-1594`); the model retrieves it via the `get_task_output` tool (`GB/src/implementations/grok_build/task_output/mod.rs:750`) — the retrieval hint reads exactly `"Use {tool} tool with task_ids=[\"{id}\"] to retrieve the output."` (`bash/mod.rs:2005-2008`; renderer fallback name `"get_command_or_subagent_output"`, `bash/mod.rs:1994-1996`).
  - *Kill:* `kill_task` tool (`kill_task/mod.rs:158`); SIGTERM→SIGKILL escalation in the actor poll loop (`terminal.rs:1224,1501-1521`); toolset finalize *requires* BackgroundTaskAction + KillTaskAction tools whenever backgrounding is enabled (`bash/mod.rs:1558-1573`).
  - *Completion notice:* newly-completed bg tasks are surfaced as `<system-reminder>` text on the next tool result (`GB/src/reminders/task_completion.rs:1-14,608-615`; gated by `surface_bg_completion_reminders`, `bash/mod.rs:199-203`).
  - *Auto-background:* opt-in `auto_background_on_timeout` moves a foreground command to background at `min(timeout, 15s FG block budget)` instead of killing (`bash/mod.rs:164-181`, budget `terminal.rs:52-62,1623-1639`); result surfaces as `BackgroundTaskStarted` with summary `"Command \"{cmd}\" exceeded the default timeout and was automatically moved to background. Process is still running."` (`bash/mod.rs:2073-2131`), and a user Ctrl+G "backgrounded" run renders the verbose `[Command moved to background]…` prompt (`bash/mod.rs:418-431`).
  - Ours: none of the above — foreground only, dropped by design in v0 (`terminal.rs:1-2,28`).
- **Command validation guardrails.** GB rejects (as invalid-arguments errors, before spawn): (1) a background `&` operator in foreground commands *when the escape hatches are off* — note defaults `allow_background_operator: true` + `enabled_background: true` mean the rejection is OFF in a default grok_build session (`bash/mod.rs:191-197,602-614`); when it fires, exact messages e.g. `"Remove the background '&' from your command and set is_background=true instead."` (`bash/mod.rs:1455-1476`); detection is quote/heredoc-aware and exempts trailing `wait` (`bash/mod.rs:622-839`); (2) self-matching `pkill -f`/`pgrep -f` patterns, with a long exact remediation message ("self-matching {cmd}/-f: …", `bash/mod.rs:1887-1899`, detection `bash/mod.rs:861-958`); (3) `is_background` with backgrounding disabled: `"Background execution is disabled."` (`bash/mod.rs:1901-1905`). Ours: no validation of any kind (`terminal.rs:92-121`).
- **Error messages.** GB spawn/backend failure: `"Failed to spawn command: {0}"` / `"Command execution failed: {0}"` / `"Terminal error: {0}"` (`bash/mod.rs:45-55`). Ours: host's `"failed to spawn shell: {0}"` passed through as a soft error (`shell.rs:50-54`, `terminal.rs:109-113`). Different text.
- **Cancellation.** GB: user/model cancellation flows through `kill_task` or actor shutdown → synthesized signal strings `"cancelled"` / `"killed"` → header `exit: killed (cancelled)` etc. (`bash/mod.rs:341-353`, `terminal.rs:1246-1266`). Ours: cooperative `CancellationToken` group-kills the child (`shell.rs:135-138,205-234`); the `cancelled` flag on `ExecOutput` (`shell.rs:39-40`) is **dropped** by the tool — a cancelled run presents as plain `exit: killed` (`terminal.rs:63-67,115-120`).
- **Other GB-only runtime features** (not model-visible schema, but behavior): per-tick streaming `ToolProgress` deltas capped at 16 KiB/frame (`bash/mod.rs:67,72-101,1622-1771`); `cmd_prefix` param prepends `"{prefix} {sep} {command}"` (`bash/mod.rs:1499-1508`); bare-`echo`/`printf` detection for telemetry (`bash/mod.rs:997-1083,2175-2183`); git commit/PR detection spans (`bash/mod.rs:2185-2204`); Linux child-network seccomp filter under sandbox (`terminal.rs:2768-2773`). Ours: none.

## Quirks

Things Grok Build does that a faithful port must reproduce exactly (or consciously defer):

1. **Two different `timeout` descriptions exist** — the raw struct attr (with "(2 minutes)") and the exported wire form (without it, numbers interpolated, `maximum` added only when a session ceiling is configured). The wire form is the faithful target (`bash/mod.rs:1351-1374`).
2. **Lenient argument parsing**: `timeout` accepts JSON string numbers; `is_background` accepts `"true"/"yes"/"1"/1` (`bash/mod.rs:263-271,284-288`). Models actually emit these.
3. **Exact truncation strings**: in-body separator `"\n\n... (output truncated) ...\n\n"` (with `trim_end`/`trim_start` on the halves) and header annotation `" [truncated: showing first/last {X} of {Y} - full output at: {path}]"` with `format_bytes` (`"…B"/"…KB"/"…MB"`, 1 decimal, decimal units).
4. **`exit:` header hides sentinel `-1` for synthesized kills**: `exit: killed (timeout)`, `(cancelled)`, `(max_runtime)`, `(signal 9)` — and suppresses the redundant `[signal=…]` annotation for those; only unrecognized signal strings surface as `exit: -1 [signal=…]` (`bash/mod.rs:329-343,385-443`).
5. **ANSI-strip + 2000-char soft-wrap** of the prompt body (`output.rs:453-458`); raw bytes are preserved separately for the UI.
6. **Truncation counts chars, not bytes**, against a 20k limit, split 10k front / 10k back (`terminal.rs:346-380`).
7. **`&`-rejection is dormant by default** (`allow_background_operator` defaults `true`); it only bites when backgrounding is disabled — which is exactly our v0 configuration, so a faithful v0 would *reject* `&` with the "(false, false)" message `"Remove the background '&' from your command; background execution is disabled."` (`bash/mod.rs:191-197,602-614,1472-1474`).
8. **`pkill -f` self-match rejection** with its very long exact message (`bash/mod.rs:1889-1897`).
9. **PYTHONUNBUFFERED=1** injected for background runs only (`bash/mod.rs:1922-1926`).
10. **Zsh `-o nonomatch`** when the detected shell is zsh (`terminal.rs:2726-2728`).
11. `description` arg is mandatory but has zero execution effect (UX/notification only).

## Fixing task

Scope estimate: **M** for the faithful-foreground slice (criteria 1–8), **L** if background mode (criteria 9–11) is included — background needs new `locode-host` surface and two new tools (`get_task_output`, `kill_task`), which is its own task.

Host-seam constraints: `locode-host` today offers one-shot `exec` with a hard timeout, per-stream tail byte caps, and cooperative cancel (`shell.rs:65-157`). It has **no** process registry, no output-to-file streaming, and no front/back char truncation. Criteria 6–7 need host changes; 9–11 need a new host surface.

1. **Timeout wire description**: change the `timeout` schemars description to the exported wire form verbatim: `"Optional timeout in milliseconds (max 300000). Default: 120000. `timeout: 0` in background mode disables the wrapper timeout entirely; the task runs until it exits or is killed via the kill task tool."` (drop "(2 minutes)"). Accept: schema snapshot equals GB `bash/mod.rs:1367-1370` rendering. (If we keep `is_background` dropped, decide explicitly whether to also strip the bg-zero sentence and note the deviation in the ADR — faithful text references a param we don't expose.)
2. **Lenient `timeout` parsing**: accept integer or numeric-string. Accept: `{"timeout":"120000"}` parses to 120000; `null`/absent → default (mirrors GB tests `bash/mod.rs:2232-2253`).
3. **Tool description**: replace the one-liner with GB's rendered default-unix description (background-disabled variant `bash/mod.rs:1440-1453` is the honest match for our v0: no `is_background` bullet), with 300000/120000/20000 interpolated and the SIGTERM→SIGKILL sentence included. Accept: byte-identical to the rendered GB template for a `enabled_background:false` unix session.
4. **Truncation fidelity (prompt shape)**: 20,000-char cap, front/back 10k+10k, exact separator `"\n\n... (output truncated) ...\n\n"`, header annotation `" [truncated: showing first/last {X} of {Y}…]"` (the `- full output at:` clause depends on criterion 7; if no output file, decide and document). Accept: a 100k-char output yields first/last 10k joined by the exact separator, header carries `format_bytes`-formatted sizes.
5. **Exit-header reasons**: reproduce `exit: killed (timeout)` / `(cancelled)` / `(signal N)` and the `[signal=…]` annotation rule; wire `ExecOutput.cancelled` (currently dropped at `terminal.rs:115-120`) into `(cancelled)`. Accept: timeout, cancel, and signal-kill each render GB's exact header.
6. **Host: combined interleaved capture + char-based front/back cap** — new host surface: extend `ExecOutput` with a single combined stream captured in arrival order (or an ordered chunk list) and front/back retention (keep-first + keep-last under a char budget) instead of tail-only per-stream byte caps; keep `total_bytes`. Accept: host test shows first- and last-half retention and a true total byte count.
7. **Host: output file** (optional but needed for the full annotation text): stream full output to a per-call log under a session dir; expose the path in `ExecOutput`. Accept: truncated runs report an existing file containing the full (≤64 MiB-retained) output.
8. **ANSI-strip + soft-wrap** the prompt body (strip escapes; wrap lines >2000 chars). Accept: colored output renders clean; a 5000-char line is wrapped, content preserved.
9. **Background mode — host surface (new)**: `Host::exec_background(ExecRequest) -> BgHandle{task_id, pid, output_file}` plus a host-owned task registry with: poll/snapshot (status, exit, tail), SIGTERM→grace→SIGKILL kill-by-task-id, 10h max-runtime reaper, and completion events the engine can drain. This is precisely what GB's terminal actor provides (`terminal.rs:1209-1232,1501-1521`); our current one-shot `exec` cannot express it.
10. **Background mode — schema/tools**: restore `is_background` (exact description, lenient bool, `bash/mod.rs:279-288`), return the `<task-id>…` XML envelope (`output.rs:775-793`) with the exact retrieval-hint sentence, and port `get_task_output` + `kill_task` (GB finalize *requires* them whenever the param is exposed, `bash/mod.rs:1558-1573`). Accept: `is_background:true` returns the envelope immediately; output retrievable; kill works.
11. **Guardrails**: port the `&`-operator rejection (active in our bg-disabled config — see Quirk 7) with exact messages, the `wait`-suffix/heredoc/quote exemptions, and the self-matching `pkill/pgrep -f` rejection with GB's exact message. Accept: `sleep 5 &` → GB's "(false,false)" message; `cmd1 & cmd2 & wait` passes; `pkill -f my_script.py ; ./my_script.py` → rejected with the pkill message.
12. **ADR reconciliation** (repo rule: ADR-first): record which criteria land now vs deferred (esp. background mode and the `-lc` vs `-c` + PATH-probe shell choice) as a dated amendment to ADR-0012 before code changes.

## Split: immediate vs deferred (user decision, 2026-07-20)

**Deferred — background mode** (user call): criteria 9–10 (the
`Host::exec_background` surface + task registry, `is_background` schema field,
`<task-id>` envelope, and the `get_task_output`/`kill_task` companion tools).

**Immediate (faithful foreground slice):** criteria 1–5, 8, 11, 12 — including
criterion 11's trailing-`&` rejection, which is **not** background work: GB
actively rejects `&` exactly in the background-disabled configuration we now
permanently occupy until the deferred slice lands. Criterion 3 uses GB's
background-disabled description variant (`bash/mod.rs:1440-1453`), which is
the honest rendering for our configuration. Criterion 1's timeout description:
verify at implementation time which text GB's wire carries in a
background-disabled session and use that verbatim.

**Immediate host work:** criterion 6 (combined interleaved capture +
front/back char cap — truncation fidelity depends on it) and criterion 7
(full-output spill file; if skipped initially, the `- full output at:` clause
must be omitted with the deviation documented, per criterion 4's note).

Immediate scope: **M** (incl. the two host changes); deferred scope: **L**.
