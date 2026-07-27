# Harness study — CLI arguments across the four reference CLIs

> **Source freshness.** Last verified against the `coding-cli-survey` submodules:
> **2026-07-22** (the newest dated note below — this stamp was inferred from the
> document's own history, not from a re-read on 2026-07-27).
> Submodule commits as of 2026-07-27: `claude-code` 6a25909 · `codex` f201c30c · `grok-build` b189869 · `opencode` 1754480.
>
> `AGENTS.md` requires a fresh source re-read when planning each task
> ([`autonomous-workflow.md`](../autonomous-workflow.md) Phase 1). **Update this line
> — date and commits — in the same PR as that re-read.** Without it a reader cannot tell
> whether the `file:line` citations below still point at what they claim — which is how a
> wrong injection point survived months in the subagent study (corrected 2026-07-26, #240).

Source study of the command-line surfaces of **Claude Code** (Commander /
TypeScript), **Codex** (`codex-rs`, clap), **Grok Build** (`xai-grok-pager`,
clap), and **opencode** (yargs / TypeScript), conducted 2026-07-22 against the
`coding-cli-survey` submodules. Citations are `harness: path:line`, relative to
each submodule root. This document feeds the `locode` CLI seam (`locode-exec` /
`locode-tui`) and the unified-binary work (Task 28).

Method: locate each harness's top-level arg parser, read every user-facing flag
(and the load-bearing hidden ones), record flag / purpose / default / mode
(interactive · headless · both), then cross-compare and distil a prioritized
port list for locode. Where a flag's behavior is non-obvious (Claude Code's
`--bare`, Codex's `--sandbox`/`--ask-for-approval` split, Grok's sticky
`--minimal`), the "why" and "why-not-the-obvious-alternative" are called out.

---

## Scope

- **In scope:** the *root* CLI of each harness — the flags a user types to start
  an interactive session or a headless one-shot. This is exactly the surface
  `locode`'s single binary must decide to mirror or reject.
- **Focus areas** (per the task): mode & I/O (`-p`, output/input format,
  partial-message streaming); model/provider selection and reasoning/thinking;
  session continuity (`continue`/`resume`/`fork`/session-id); context injection
  (`add-dir`, system-prompt, settings, mcp-config, agents); permissions & safety
  (permission-mode, skip-permissions, allowed/disallowed/tools); the
  "minimal/fast" mode (Claude Code's **`--bare`** and analogs); power-user knobs
  (`--max-turns`, `--max-budget-usd`, debug, verbose).
- **Out of scope (noted, not enumerated exhaustively):** management subcommands
  (`mcp`, `plugin`, `auth`/`login`, `serve`, `github`, `pr`, `stats`, `doctor`,
  `completion`, worktree/tmux orchestration, remote/teleport, SDK-daemon and
  teammate/swarm flags). These are real but orthogonal to the core+TUI arg
  surface; a few are cited where they carry a lesson.

---

## Per-harness arg inventories

### 1. Claude Code — Commander, all options on one chain

Every root option is defined on a single Commander chain in **`src/main.tsx`**
(`new CommanderCommand()` at `claude-code: src/main.tsx:902`; `program.name('claude')`
at `:968`). Claude Code is **interactive by default**; `-p/--print` flips it to
headless (`:976`). The `[prompt]` positional pre-fills the composer interactively
or is the task under `-p` (`:968`). Below, "line" is the `src/main.tsx` line the
option is defined on.

**Mode & I/O**

| Flag | Purpose | Default | Mode | Line |
|---|---|---|---|---|
| `[prompt]` | Positional prompt | — | both | 968 |
| `-p, --print` | Print response and exit (non-interactive). Skips workspace-trust dialog. | off | flips to headless | 976 |
| `--output-format <text\|json\|stream-json>` | Headless output shape (`--print` only) | `text` | headless | 976 |
| `--input-format <text\|stream-json>` | Streaming input (`--print` only) | `text` | headless | 976 |
| `--include-partial-messages` | Emit partial chunks as they arrive (needs `--print` + `stream-json`) | off | headless | 976 |
| `--include-hook-events` | Emit hook lifecycle events (needs `stream-json`) | off | headless | 976 |
| `--json-schema <schema>` | Constrain structured output to a JSON Schema | — | headless | 976 |
| `--replay-user-messages` | Re-emit stdin user msgs back on stdout (stream-json in+out) | off | headless | 988 |
| `--no-session-persistence` | Don't save session to disk (`--print` only) | persist | headless | 991 |

**Model / provider / thinking**

| Flag | Purpose | Default | Mode | Line |
|---|---|---|---|---|
| `--model <model>` | Model alias (`sonnet`,`opus`) or full name | config | both | 993 |
| `--fallback-model <model>` | Auto-fallback when default overloaded (`--print` only) | — | headless | 1000 |
| `--effort <low\|medium\|high\|max>` | Effort level for the session | — | both | 993 |
| `--thinking <enabled\|adaptive\|disabled>` | Thinking mode (hidden) | — | both | 976 |
| `--max-thinking-tokens <n>` | Deprecated thinking-token cap (`--print` only, hidden) | — | headless | 976 |
| `--betas <betas...>` | Beta headers (API-key users only) | — | both | 1000 |

> Provider selection in Claude Code is **not** a flag: Anthropic direct vs.
> Bedrock/Vertex/Foundry is chosen via env (`ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_BEDROCK`,
> etc.) and `--settings`. `--bare` narrows auth to `ANTHROPIC_API_KEY`/`apiKeyHelper`
> only (see below). This is the closest analog to our `--api-schema`, but Claude
> keeps it out of the CLI surface.

**Session continuity**

| Flag | Purpose | Default | Mode | Line |
|---|---|---|---|---|
| `-c, --continue` | Continue most recent conversation in cwd | off | both | 988 |
| `-r, --resume [value]` | Resume by session id, or interactive picker w/ optional search | picker | both | 988 |
| `--fork-session` | On resume, mint a NEW session id (with `-r`/`-c`) | off | both | 988 |
| `--session-id <uuid>` | Use a specific UUID for the conversation | — | both | 1000 |
| `-n, --name <name>` | Display name (shown in `/resume`, terminal title) | — | both | 1000 |
| `--resume-session-at <msg id>` | Resume only up to a message id (with `-r`, print mode; hidden) | — | headless | 991 |
| `--from-pr [value]` | Resume a session linked to a PR | picker | both | 991 |
| `--rewind-files <user-msg-id>` | Restore files to a message's state and exit (needs `--resume`; hidden) | — | headless | 991 |

**Context injection**

| Flag | Purpose | Default | Mode | Line |
|---|---|---|---|---|
| `--add-dir <dirs...>` | Extra directories tools may access | — | both | 1000 |
| `--system-prompt <prompt>` | Replace the session system prompt | — | both | 988 |
| `--system-prompt-file <file>` | Same, from a file (hidden) | — | both | 988 |
| `--append-system-prompt <prompt>` | Append to the default system prompt | — | both | 988 |
| `--append-system-prompt-file <file>` | Append from a file (hidden) | — | both | 988 |
| `--settings <file-or-json>` | Load additional settings (path or JSON string) | — | both | 1000 |
| `--setting-sources <user,project,local>` | Which setting sources to load | all | both | 1000 |
| `--mcp-config <configs...>` | Load MCP servers from JSON files/strings | — | both | 988 |
| `--strict-mcp-config` | Use ONLY `--mcp-config`, ignore other MCP config | off | both | 1000 |
| `--agents <json>` | Inline custom-agent definitions (JSON) | — | both | 1000 |
| `--agent <agent>` | Select an agent for the session | — | both | 1000 |
| `--plugin-dir <path>` | Load plugins from a dir (repeatable) | — | both | 1006 |
| `--file <specs...>` | Download file resources at startup (`file_id:path`) | — | both | 1006 |

**Permissions & safety**

| Flag | Purpose | Default | Mode | Line |
|---|---|---|---|---|
| `--permission-mode <mode>` | Session permission mode (choices = `PERMISSION_MODES`) | default | both | 988 |
| `--dangerously-skip-permissions` | Bypass ALL permission checks | off | both | 976 |
| `--allow-dangerously-skip-permissions` | Make skip available as a toggle without enabling it | off | both | 976 |
| `--allowedTools, --allowed-tools <tools...>` | Allowlist (e.g. `Bash(git:*) Edit`) | — | both | 988 |
| `--disallowedTools, --disallowed-tools <tools...>` | Denylist | — | both | 988 |
| `--tools <tools...>` | Which built-in tools exist (`""`=none, `default`=all, or names) | default | both | 988 |
| `--permission-prompt-tool <tool>` | MCP tool for permission prompts (`--print` only, hidden) | — | headless | 988 |

**The "minimal/fast" mode — `--bare` (`claude-code: src/main.tsx:976`)**

> `--bare` — *"Minimal mode: skip hooks, LSP, plugin sync, attribution,
> auto-memory, background prefetches, keychain reads, and CLAUDE.md
> auto-discovery. Sets `CLAUDE_CODE_SIMPLE=1`. Anthropic auth is strictly
> `ANTHROPIC_API_KEY` or `apiKeyHelper` via `--settings` (OAuth and keychain are
> never read). 3P providers (Bedrock/Vertex/Foundry) use their own credentials.
> Skills still resolve via `/skill-name`. Explicitly provide context via:
> `--system-prompt[-file]`, `--append-system-prompt[-file]`, `--add-dir`
> (CLAUDE.md dirs), `--mcp-config`, `--settings`, `--agents`, `--plugin-dir`."*

What `--bare` disables, precisely: **hooks, LSP, plugin auto-sync, git/commit
attribution, cross-session auto-memory, background prefetches, OS-keychain
reads, and CLAUDE.md auto-discovery**. What it *keeps*: skills (still resolvable
by `/name`), and all the explicit context flags — so a caller composes context
by hand instead of paying for auto-discovery. **Why:** deterministic,
low-latency, side-effect-free startup for scripting/CI/SDK — no filesystem walk
for CLAUDE.md, no keychain popup, no plugin network sync, no auto-memory
mutation. **Why not just document env vars:** one named flag is discoverable and
atomic; it also *pins auth* to `ANTHROPIC_API_KEY`/`apiKeyHelper` so a scripted
run can't silently pick up an interactive OAuth token. This is the single most
relevant precedent for a locode `--bare`-style pack-only fast path.

**Power-user / debug**

| Flag | Purpose | Default | Mode | Line |
|---|---|---|---|---|
| `--max-turns <n>` | Cap agentic turns, early-exit (`--print` only, hidden) | ∞ | headless | 976 |
| `--max-budget-usd <amount>` | Cap total API spend (`--print` only, hidden) | ∞ | headless | 976 |
| `-d, --debug [filter]` | Debug mode w/ category filter (`api,hooks` / `!file`) | off | both | 971 |
| `--debug-to-stderr` / `--debug-file <path>` | Route debug output | — | both | 976 |
| `--verbose` | Override config verbose | off | both | 976 |
| `-w, --worktree [name]` / `--tmux` | Session in a git worktree / tmux | — | interactive | 3811 |
| `--ide` / `--chrome` / `--no-chrome` | IDE / Chrome integrations | off | interactive | 1000 |
| `--disable-slash-commands` | Disable all skills | off | both | 1006 |
| `--prefill <text>` | Pre-fill composer without submitting (hidden) | — | interactive | 988 |

Claude Code also carries a large `[ANT-ONLY]`/hidden surface (teammate/swarm,
tasks mode, remote/teleport, remote-control) at `:3811–3871` — orthogonal to
core+TUI, noted for completeness.

---

### 2. Codex — clap, split across a shared struct + interactive/exec

Codex's root parser is `MultitoolCli` (`codex: codex-rs/cli/src/main.rs:106`),
which **flattens** four groups then optionally dispatches a subcommand:
`CliConfigOverrides` (`-c`), `FeatureToggles` (`--enable`/`--disable`),
`InteractiveRemoteOptions` (`--remote`), and `TuiCli` (the default interactive
session). Headless is the **`codex exec`** subcommand (`:126`), whose parser is
`exec/src/cli.rs`. Both interactive and exec share `SharedCliOptions`
(`utils/cli/src/shared_options.rs:9`) — the model/sandbox/cwd core.

**Shared core — `SharedCliOptions` (both interactive & exec)**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `-m, --model <model>` | Model to use | config | both | shared_options.rs:21 |
| `--oss` | Use open-source (local) provider | off | both | :25 |
| `--local-provider <lmstudio\|ollama>` | Which local provider (with `--oss`) | — | both | :30 |
| `-p, --profile <name>` | Layer `$CODEX_HOME/<name>.config.toml` on base config | — | both | :34 |
| `-s, --sandbox <read-only\|workspace-write\|danger-full-access>` | Sandbox policy for model commands | config | both | :39 |
| `--dangerously-bypass-approvals-and-sandbox` (alias `--yolo`) | Skip prompts AND sandbox | off | both | :44 |
| `--dangerously-bypass-hook-trust` | Run hooks without persisted trust | off | both | :53 |
| `-C, --cd <dir>` | Working root | cwd | both | :57 |
| `--add-dir <dir>` | Extra writable dirs (repeatable) | — | both | :61 |
| `-i, --image <file,...>` | Images for the initial prompt | — | both | :11 |

**Interactive — `TuiCli` (`tui/src/cli.rs:10`)**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `[PROMPT]` | Optional starting prompt | — | interactive | :12 |
| `-a, --ask-for-approval <untrusted\|on-request\|never>` | When to require human approval | config | interactive | :61 |
| `--search` | Enable native `web_search` tool (no per-call approval) | off | interactive | :65 |
| `--no-alt-screen` | Inline mode, preserve scrollback | off | interactive | :71 |
| `--strict-config` | Error on unknown `config.toml` fields | off | interactive | :16 |

> **Codex's approval model is two orthogonal axes**, deliberately:
> `--ask-for-approval` (*when to ask*) × `--sandbox` (*what's allowed without
> asking*). `--yolo` = "never ask + full access" collapses both. This is a
> cleaner factoring than Claude's single `--permission-mode`, and worth studying
> for locode's approval seam. `ApprovalModeCliArg` = `Untrusted`/`OnRequest`/`Never`
> (`utils/cli/src/approval_mode_cli_arg.rs:9`); `SandboxModeCliArg` =
> `ReadOnly`/`WorkspaceWrite`/`DangerFullAccess` (`sandbox_mode_cli_arg.rs:14`).

**Headless — `codex exec` (`exec/src/cli.rs:14`)**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `[PROMPT]` | Prompt; `-` or absent reads stdin; piped stdin appended as `<stdin>` | stdin | headless | :84 |
| `--json` (alias `--experimental-json`) | Print events to stdout as JSONL | off | headless | :64 |
| `--output-schema <file>` | JSON-Schema file for final response shape | — | headless | :52 |
| `-o, --output-last-message <file>` | Write agent's last message to a file | — | headless | :72 |
| `--color <auto\|always\|never>` | Color settings | auto | headless | :59 |
| `--skip-git-repo-check` | Allow running outside a git repo | off | headless | :26 |
| `--ephemeral` | Don't persist session files | off | headless | :30 |
| `--ignore-user-config` | Don't load `config.toml` (auth still uses `CODEX_HOME`) | off | headless | :34 |
| `--ignore-rules` | Don't load execpolicy `.rules` files | off | headless | :38 |
| `--strict-config` | Error on unknown config fields | off | headless | :19 |

> Codex's exec has **no `--max-turns`** and **no output-format enum** — headless
> is `--json` (JSONL events) or human text, and structured output is a *schema
> file* (`--output-schema`), not an inline string. `--ephemeral` +
> `--ignore-user-config` + `--ignore-rules` together are Codex's *decomposed*
> answer to Claude's `--bare`: no single flag, but the same "strip startup
> side-effects for scripting" intent expressed as three targeted opt-outs.

**Root-level cross-cutting**

| Flag | Purpose | Default | Cite |
|---|---|---|---|
| `-c, --config <key=value>` | Override any config value (dotted path, TOML-parsed, repeatable) | — | utils/cli/src/config_override.rs:29 |
| `--enable <feature>` / `--disable <feature>` | Feature toggle = `-c features.<name>=…` (repeatable) | — | cli/src/main.rs:877 |
| `--remote <addr>` / `--remote-auth-token-env <env>` | Connect TUI to a remote app-server | — | cli/src/main.rs:888 |

> **`-c key=value` is Codex's escape hatch**: any config field is a CLI override
> with lower precedence than dedicated flags, TOML-parsed on the RHS. This is why
> Codex needs fewer bespoke flags than Claude — the entire `config.toml` surface
> is reachable generically. Subcommands: `exec`, `resume`, `fork`, `archive`,
> `delete`, `unarchive`, `review`, `apply`, `cloud`, `login`/`logout`, `mcp`,
> `mcp-server`, `app-server`, `doctor`, `completion`, `update` (`main.rs:124`).
> Continuity is subcommand-shaped (`codex resume [id] [--last] [--all]`,
> `codex fork`) not root-flag-shaped — see `resume`/`fork` at `main.rs:312/374`.

---

### 3. Grok Build — clap, one big `PagerArgs` for the unified binary

Grok's single binary (`grok`) parses `PagerArgs`
(`grok-build: crates/codegen/xai-grok-pager/src/app/cli.rs:404`), interactive by
default, headless via `-p/--single`. Notably **`-p`'s canonical name is
`--single`** with `--print` as an alias (`:457`) — the flag *carries the prompt*
(`-p "task"`), unlike Claude/locode where `-p` is a boolean. Provider base-URL
overrides live on the `agent` subcommand (`AgentArgs`, `:226`), not the root.

**Mode & I/O**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `[PROMPT]` | Interactive initial prompt | — | interactive | :715 |
| `-p, --single <PROMPT>` (alias `--print`) | Single-turn headless; print response and exit | — | headless | :457 |
| `--prompt-json <JSON>` | Single-turn prompt as JSON content blocks | — | headless | :467 |
| `--prompt-file <path>` | Single-turn prompt from a file | — | headless | :475 |
| `--verbatim` | Send the prompt exactly as given | off | headless | :484 |
| `--output-format <plain\|json\|streaming-json>` | Headless output shape | `plain` | headless | :487 |
| `--json-schema <schema>` | Structured output schema (implies `--output-format json`) | — | headless | :492 |

**Model / reasoning / provider**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `-m, --model <model>` | Model id | config | both | :495 |
| `--reasoning-effort <effort>` (alias `--effort`) | Reasoning effort | config | both | :498 |
| `--xai-api-base-url <url>` | Override xAI API base URL (on `agent` subcmd) | — | both | :271 |
| `--cli-chat-proxy-base-url <url>` | Override chat-proxy base URL (on `agent` subcmd) | — | both | :268 |

**Context injection**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `--rules <text>` (alias `--append-system-prompt`) | Extra rules appended to system prompt | — | both | :506 |
| `--system-prompt-override <p>` (alias `--system-prompt`) | Replace the system prompt | — | both | :519 |
| `--agent <name>` | Agent name or definition-file path | — | both | :588 |
| `--agents <json>` | Inline subagent definitions (JSON) | — | both | :591 |
| `--agent-profile <path>` | Agent profile (on `agent` subcmd) | — | both | :249 |
| `--plugin-dir <dir>` | Plugin dirs (on `agent` subcmd, repeatable) | — | both | :255 |

> Grok has **no `--add-dir`** and **no `--settings`/`--mcp-config`** on the root
> — MCP is a subcommand (`mcp_cmd.rs`), settings live in `~/.grok/config.toml`,
> and workspace access is governed by `--sandbox` profiles rather than an
> allow-list of dirs. Its context surface is thinner than Claude's by design.

**Permissions & safety**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `--always-approve` (aliases `--yolo`, `--dangerously-skip-permissions`) | Auto-approve all tool executions | off | both | :432 |
| `--permission-mode <mode>` | Permission mode (validated against `PermissionMode::VALID_VALUES`) | config | both | :607 |
| `--allow <rule>` (alias `--allowedTools`) | Allow rule (comma-sep) | — | both | :441 |
| `--deny <rule>` (alias `--disallowedTools`) | Deny rule (comma-sep) | — | both | :449 |
| `--tools <tools>` | Built-in tools to allow (comma-sep) | all | both | :594 |
| `--disallowed-tools <tools>` | Built-in tools to remove | — | both | :597 |
| `--sandbox <profile>` (env `GROK_SANDBOX`) | Filesystem/network sandbox profile | config | both | :645 |
| `--trust` (alias `--trust-folder`, hidden) | Trust this folder, persist decision | off | both | :438 |
| `--disable-web-search` | Disable web search + fetch tools | off | both | :616 |

> Grok mirrors **Claude Code's exact permission flag names as aliases**
> (`--allowedTools`, `--disallowedTools`, `--dangerously-skip-permissions`) —
> deliberate cross-CLI muscle-memory compatibility. Its own canonical names are
> the shorter `--allow`/`--deny`/`--yolo`. A concrete precedent for locode
> keeping compat aliases on our canonical flags.

**Session continuity**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `-r, --resume [SESSION_ID]` | Resume by id, or most recent if omitted | picker/recent | both | :526 |
| `-c, --continue` | Continue most recent session for cwd | off | both | :544 |
| `-s, --session-id <uuid>` | Use a specific UUID for a **new** session | — | both | :556 |
| `--fork-session` | On resume/continue, mint a new id | off | both | :560 |
| `--load <id>` (hidden alias for `--resume`) | Resume by id | — | both | :536 |
| `--restore-code` | Check out the session's original commit on resume | off | both | :570 |
| `-w, --worktree [name]` / `--worktree-ref <ref>` | Session in a git worktree | — | both | :563 |

**Power-user / turns / mode**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `--max-turns <N>` | Cap agent turns (range ≥1) | ∞ | both | :600 |
| `--best-of-n <N>` | Run N ways in parallel, pick best (headless) | — | headless | :645 |
| `--check` (alias `--self-verify`) | Append a self-verification loop (headless) | off | headless | :619 |
| `--no-wait-for-background` / `--background-wait-timeout <secs>` | Background-task waiting after first turn (headless, hidden) | wait/600s | headless | :627/:635 |
| `--no-plan` / `--no-subagents` / `--no-ask-user` | Disable plan mode / subagents / structured questions | on | both | :573/:576/:579 |
| `--experimental-memory` / `--no-memory` | Cross-session memory toggle | off | both | :582/:585 |
| `--minimal` | Scrollback-native rendering; **sticky** (writes `[ui] screen_mode="minimal"`) | off | interactive | :690 |
| `--fullscreen` | Standard fullscreen TUI; sticky counterpart of `--minimal` | off | interactive | :698 |
| `--no-alt-screen` | Inline instead of alt-screen | off | interactive | :683 |
| `--debug` / `--debug-file <file>` | Debug logging / route to file | off | both | :420/:423 |

> **Grok's `--minimal`/`--fullscreen` are *sticky*** — they persist the choice
> into `~/.grok/config.toml` so a bare `grok` reopens in the same mode. This is a
> UX pattern (remember the last render mode), not a per-run flag. Relevant to
> locode's TUI render-mode selection but firmly TUI-only.

---

### 4. opencode — yargs, thin root + config-file-driven

opencode's root (`opencode: packages/opencode/src/index.ts:45`) is deliberately
minimal: three global options plus a fan-out of subcommands. The **default
command `$0`** launches the TUI (`packages/opencode/src/cli/cmd/tui.ts:73`);
headless is the **`run`** subcommand (`packages/opencode/src/cli/cmd/run.ts:127`).
Most configuration (providers, MCP, agents, permissions) lives in
`opencode.json`, **not** flags — the thinnest CLI of the four.

**Global (all commands) — `index.ts`**

| Flag | Purpose | Default | Cite |
|---|---|---|---|
| `--print-logs` | Print logs to stderr | off | :53 |
| `--log-level <DEBUG\|INFO\|WARN\|ERROR>` | Log level | — | :57 |
| `--pure` | Run without external plugins | off | :62 |
| `-h/--help`, `-v/--version`, `completion` | Standard | — | :48 |

> `--pure` (skip external plugins) is opencode's smallest `--bare`-adjacent
> idea; combined with `--print-logs`/`--log-level` it's the whole "deterministic
> scripting" surface. Everything else is config-file.

**Interactive TUI — default `$0 [project]` (`cli/cmd/tui.ts:73`)**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `[project]` | Path to start opencode in | cwd | interactive | :77 |
| `-m, --model <provider/model>` | Model as `provider/model` | config | interactive | :81 |
| `-c, --continue` | Continue the last session | off | interactive | :86 |
| `-s, --session <id>` | Session id to continue | — | interactive | :91 |
| `--fork` | Fork the session when continuing (with `-c`/`-s`) | off | interactive | :96 |
| `--prompt <text>` | Prompt to use | — | interactive | :100 |
| `--agent <name>` | Agent to use | — | interactive | :104 |
| `--auto` | Auto-approve permissions not explicitly denied (dangerous) | off | interactive | :108 |
| `--yolo` / `--dangerously-skip-permissions` (hidden) | Aliases folded into auto-approve | off | interactive | :113/:118 |
| `--mini` | Start the minimal interactive interface | off | interactive | :123 |
| `--no-replay` / `--replay-limit <N>` | Mini history replay controls | replay on | interactive | :132/:136 |
| network: `--port`/`--hostname`/`--mdns`/`--mdns-domain`/`--cors` | Server binding | 0/127.0.0.1 | interactive | network.ts:6 |

**Headless — `run [message..]` (`cli/cmd/run.ts:127`)**

| Flag | Purpose | Default | Mode | Cite |
|---|---|---|---|---|
| `[message..]` | Message to send (array) | — | headless | :137 |
| `--command <name>` | Run a named command, message = args | — | headless | :143 |
| `-c, --continue` / `-s, --session <id>` / `--fork` | Continuity (same as TUI) | off | headless | :147/:152/:157 |
| `--share` | Share the session | off | headless | :161 |
| `-m, --model <provider/model>` | Model | config | headless | :165 |
| `--agent <name>` | Agent | — | headless | :170 |
| `--format <default\|json>` | Output: formatted or raw JSON events | `default` | headless | :174 |
| `-f, --file <file...>` | Attach file(s) to the message | — | headless | :180 |
| `--title <text>` | Session title | trunc. prompt | headless | :186 |
| `--variant <effort>` | Model variant / reasoning effort (`high`,`max`,`minimal`) | — | headless | :212 |
| `--thinking` | Show thinking blocks | off | headless | :216 |
| `-i, --interactive` | Direct interactive split-footer mode | off | both | :236 |
| `--auto` / `--yolo` / `--dangerously-skip-permissions` | Auto-approve (dangerous) | off | headless | :242 |
| `--attach <url>` | Attach to a running opencode server | — | headless | :190 |
| `-u/--username`, `-p/--password` | Basic auth for `--attach` | — | headless | :194/:199 |
| `--dir <dir>` / `--port <n>` | Working dir / local server port | cwd/random | headless | :204/:208 |

> opencode has **no `--max-turns`, no `--system-prompt`, no `--add-dir`, no
> `--mcp-config`, no `--permission-mode`** on the CLI — all of that is
> `opencode.json`. `-p` is the *password* flag here, not print (a naming clash
> worth noting: opencode's "print" is `run --format json`). Model is always
> `provider/model`, so provider selection is folded into `--model`. `--variant`
> is opencode's reasoning-effort knob.

---

## Comparison table (equivalent args aligned)

| Concept | Claude Code | Codex | Grok Build | opencode | locode (today) |
|---|---|---|---|---|---|
| Positional prompt | `[prompt]` | `[PROMPT]` | `[PROMPT]` / `-p carries it` | `[project]` / `run [msg..]` | `prompt` |
| Headless toggle | `-p/--print` (bool) | `codex exec` subcmd | `-p/--single` (carries prompt) | `run` subcmd | `-p/--print` (bool) |
| Output format | `--output-format text\|json\|stream-json` | `--json` (JSONL) + `--output-schema` | `--output-format plain\|json\|streaming-json` | `--format default\|json` | `--output-format json\|text\|stream-json` |
| Input format | `--input-format text\|stream-json` | stdin (`-`) | `--prompt-json`/`--prompt-file` | `[message..]`/`-f` | stdin (implied) |
| Partial stream | `--include-partial-messages` | (in `--json` events) | `streaming-json` | (in `--format json`) | via `stream-json` |
| Structured schema | `--json-schema` | `--output-schema <file>` | `--json-schema` | — | — (gap) |
| Model | `--model` | `-m/--model` | `-m/--model` | `-m/--model provider/model` | — (gap) |
| Provider/base-url | env + `--settings` | `--oss`/`--local-provider`, `-c` | `--xai-api-base-url` (agent) | in `provider/model` | `--api-schema` |
| Reasoning/thinking | `--effort`, `--thinking` | (config) | `--reasoning-effort`/`--effort` | `--variant`, `--thinking` | — (gap) |
| Continue | `-c/--continue` | `codex resume --last` | `-c/--continue` | `-c/--continue` | — (gap) |
| Resume | `-r/--resume [id]` | `codex resume [id]` | `-r/--resume [id]` | `-s/--session <id>` | — (gap) |
| Fork | `--fork-session` | `codex fork` | `--fork-session` | `--fork` | — (gap) |
| Session id | `--session-id <uuid>` | (internal) | `-s/--session-id` | `-s/--session` | — (gap) |
| Working dir | `--add-dir` adds; cwd implicit | `-C/--cd` | `--cwd` | `--dir` / `[project]` | `--cwd` |
| Extra dirs | `--add-dir <dirs...>` | `--add-dir <dir>` | — (sandbox profile) | — | — (gap) |
| System prompt | `--system-prompt[-file]` | (config) | `--system-prompt-override` | — | — (gap) |
| Append prompt | `--append-system-prompt[-file]` | (config) | `--rules`/`--append-system-prompt` | — | `--strip-identity` (adjacent) |
| Settings | `--settings`, `--setting-sources` | `-c key=value`, `--profile` | config.toml | opencode.json | — |
| MCP config | `--mcp-config`, `--strict-mcp-config` | `mcp` subcmd | `mcp` subcmd | config | — |
| Agents | `--agents`, `--agent` | (config) | `--agents`, `--agent` | `--agent` | `--harness` (pack, adjacent) |
| Permission mode | `--permission-mode` | `-a/--ask-for-approval` × `-s/--sandbox` | `--permission-mode` | `--auto` | — (gap) |
| Skip permissions | `--dangerously-skip-permissions` | `--yolo`/`--dangerously-bypass…` | `--yolo`/`--dangerously-skip-permissions` | `--yolo`/`--dangerously-skip-permissions` | `--dangerously-skip-permissions`/`--yolo` |
| Allow/deny tools | `--allowed-tools`/`--disallowed-tools` | (execpolicy `.rules`) | `--allow`/`--deny` (+CC aliases) | (config) | — (gap) |
| Which tools | `--tools` | (config) | `--tools`/`--disallowed-tools` | (config) | — (gap) |
| Max turns | `--max-turns` | — | `--max-turns` | — | `--max-turns` |
| Max budget | `--max-budget-usd` | — | — | — | — |
| Minimal/fast | `--bare` | `--ephemeral`+`--ignore-user-config`+`--ignore-rules` | `--minimal` (render, sticky) | `--pure` (plugins), `--mini` (render) | `--strip-identity` (partial) |
| Debug/verbose | `-d/--debug`, `--verbose` | `-c`, env | `--debug`/`--debug-file` | `--print-logs`/`--log-level` | — |
| Config override | (via `--settings`) | `-c/--config key=value` | env vars | opencode.json | — |
| Inline render mode | `--worktree`, `--ide` | `--no-alt-screen` | `--minimal`/`--fullscreen`/`--no-alt-screen` | `--mini` | — |

Two vocabulary clashes to avoid: **`-p`** means *print* in Claude/locode/Grok but
*password* in opencode; **"bare/minimal"** means *skip startup side-effects*
(Claude `--bare`, opencode `--pure`) in some CLIs but *render mode* (Grok
`--minimal`, opencode `--mini`) in others. locode should keep `-p` = print and
pick a distinct word for each of the two "minimal" ideas.

---

## Pros / cons & best practice

**Headless toggle — boolean vs. prompt-carrying vs. subcommand.**
- Claude/locode `-p` boolean (prompt is positional): simplest; `-p` reads clean.
- Grok `-p/--single <PROMPT>`: `-p` *carries* the prompt — fewer positionals, but
  `-p` alone is meaningless and it collides with everyone else's boolean `-p`.
- Codex/opencode subcommand (`exec`/`run`): cleanest separation of two very
  different arg sets (headless gets `--output-schema`, `--ephemeral`, etc.
  without polluting interactive help), at the cost of one more token to type.
- **Best practice for locode:** keep the Claude-shaped boolean `-p` + positional
  prompt (already chosen, Task 28) — it's the most familiar and lets one flag set
  serve both modes. Gate headless-only flags with clap `requires`/help text, as
  Claude does ("only works with --print").

**Model & provider.** Three strategies: env-only (Claude), `provider/model` in one
flag (opencode), separate model flag + provider via config/`--oss` (Codex/Grok).
locode already splits *wire schema* (`--api-schema`) from model. **Best practice:**
add `--model` as a first-class flag (all four have it) and keep `--api-schema` as
the wire selector; do **not** fold provider into the model string (opencode's
`provider/model` couples them and our provider seam is a registry, ADR-worthy).

**Reasoning/thinking.** Every harness exposes it, but names diverge
(`--effort`/`--thinking`/`--reasoning-effort`/`--variant`). **Best practice:** one
`--reasoning-effort`/`--effort` string flag mapped per-provider; avoid a
provider-specific enum in the core.

**Session continuity.** Claude/Grok/opencode use root flags
(`-c`/`-r`/`--fork-session`); Codex uses subcommands. Root flags are lower-friction
and align with our single-binary shape. **Best practice:** port `-c/--continue`,
`-r/--resume [id]`, `--fork-session`, `--session-id` as root flags — but only once
a session-store seam exists (they're inert without persistence). Note Claude's
`--fork-session` semantics: on resume, mint a *new* id so the original is
untouched — the safe default for branching.

**Context injection.** `--add-dir` (Claude, Codex) is the load-bearing one for a
jailed core: it's how a user widens the path-jail without `--yolo`. Ties directly
to our AGENTS.md/CLAUDE.md discovery study. System-prompt flags matter less if
packs own the prompt — but `--append-system-prompt` (append user rules) is
broadly supported and low-risk; note locode's `--strip-identity` already edits the
pack prompt, so an append flag is a natural sibling.

**Permissions.** Codex's two-axis model (`--ask-for-approval` × `--sandbox`) is the
most principled; Claude's single `--permission-mode` is simpler; everyone has a
`--yolo`. locode already has `--dangerously-skip-permissions`/`--yolo` (bypass +
lift jail). **Best practice:** add `--permission-mode` (Claude/Grok shape) mapping
to our approval seam, and keep `--add-dir` as the graduated alternative to full
`--yolo`. `--allowed-tools`/`--disallowed-tools`/`--tools` are valuable but depend
on the tool-registry filtering seam.

**The `--bare`/minimal family — the sharpest lesson.** Claude's `--bare` is a
*named atomic* opt-out of all discovery/side-effects (hooks, LSP, plugins,
auto-memory, CLAUDE.md walk, keychain) + pinned auth. Codex decomposes the same
intent into `--ephemeral`/`--ignore-user-config`/`--ignore-rules`. opencode's
`--pure` covers only plugins. For locode, whose core is *already* headless and
side-effect-light, a `--bare` analog would disable: AGENTS.md/CLAUDE.md
auto-discovery, any hooks/plugins we add, and cross-session memory — leaving
explicit context flags. **Best practice:** design the seams so a single `--bare`
flag can flip them off atomically; this is an ADR-level decision because it
defines what "startup side effects" a locode pack is allowed to have.

**Config override escape hatch.** Codex's `-c key=value` (TOML-parsed, generic,
lower precedence than flags) is why Codex needs the fewest bespoke flags.
**Best practice:** consider one generic `-c/--config key=value` override rather
than growing a flag per setting — but only once locode has a config file to
override; premature without one.

**Sticky render prefs (Grok `--minimal`/`--fullscreen`).** Persisting a UI choice
is nice UX but couples the CLI to a config-write side effect — exactly the kind of
thing `--bare` then has to turn off. Keep render-mode flags *stateless* in locode
unless we deliberately adopt stickiness (and if so, `--bare` must skip the write).

---

## Recommendation for locode — prioritized port list

Legend: **Mode** = interactive (I) / headless (H) / both (B). **Seam** = what must
exist first. locode-today flags: `prompt`, `-p/--print`, `--harness`,
`--api-schema`, `--cwd`, `--max-turns`, `--output-format`,
`--dangerously-skip-permissions`/`--yolo`, `--strip-identity`.

### Must-have (port next; small seam or none)

1. **`--model <model>`** — B. All four have it; today it's a gap. Maps to the
   provider/model-selection seam (the model id passed to the wire). Low risk if
   the provider already accepts a model override. *No ADR* if it's just plumbed
   through; *ADR* if it interacts with provider-registry defaults.
2. **`--reasoning-effort` / `--effort <level>`** — B. Universal. A single string
   flag mapped per-wire (Anthropic thinking budget vs. OpenAI effort). Needs the
   wire to accept a reasoning knob. *Light ADR* on the mapping (shared string vs.
   per-provider enum).
3. **`-c/--continue`, `-r/--resume [id]`, `--fork-session`, `--session-id`** — B.
   Three of four harnesses use root flags; matches our single binary. **Blocked
   on a session-store seam** (persistence + resume). **ADR required** (session
   persistence model, fork semantics = mint-new-id like Claude). Highest-value
   cluster once the seam lands.
4. **`--append-system-prompt[-file]`** — B. Natural sibling of the existing
   `--strip-identity`; lets a user add rules without a custom pack. No new seam
   (we already mutate the pack prompt). *No ADR.*
5. **`--add-dir <dirs...>`** — B. The graduated alternative to `--yolo` for a
   jailed core: widen the path-jail without lifting it. Ties to the AGENTS.md
   discovery study and the jail seam. **ADR-adjacent** (how extra dirs compose
   with the jail). High value for safety UX.

### Nice-to-have (real value, larger seam)

6. **`--permission-mode <mode>`** — B. Claude/Grok shape over our approval seam
   (e.g. `default`/`auto`/`plan`). **ADR** to name the modes and map to the
   approval interface. Consider Codex's cleaner two-axis factoring
   (`--ask-for-approval` × sandbox) when writing it.
7. **`--allowed-tools` / `--disallowed-tools` / `--tools`** — B. Filter the tool
   registry per-run. Needs a registry-filtering seam. Port Grok's canonical names
   with Claude's `--allowedTools` aliases for muscle memory. *ADR* on rule syntax
   (bare names vs. `Bash(git:*)` scoping).
8. **`--bare` (locode analog)** — B. Atomic opt-out of AGENTS.md/CLAUDE.md
   auto-discovery + any future hooks/plugins + cross-session memory, for
   deterministic scripting/CI. **ADR required** — it *defines* what startup side
   effects a locode pack may have (see the Harness-fidelity-boundary memory:
   loop-adjacent behavior stays on the shared engine). Design other seams so one
   flag flips them.
9. **`--structured-output schema` / `--json-schema`** — H. Claude & Grok inline a
   schema; Codex uses `--output-schema <file>`. locode already has the
   single-structured-output contract — exposing a schema flag is a natural fit.
   *Light ADR* (inline string vs. file; how it binds to the report envelope).
10. **`--debug`/`--debug-file` + `--verbose`** — B. Every harness has debug
    routing. Cheap, high-utility for our own dev. No ADR. Keep library crates
    stdout-clean (debug → stderr/file only), per repo boundaries.
11. **`--no-alt-screen` / inline render mode** — I (TUI-only). Matches ADR-0022
    render work. Keep it **stateless** (do not adopt Grok's sticky write unless a
    `--bare` opt-out is wired). *No new ADR* beyond existing TUI ADRs.

### Skip (for locode's scope) — with reason

- **`-c/--config key=value` generic override** — premature: no locode config file
  to override yet. Revisit if/when one exists (Codex's model is the template).
- **`--oss`/`--local-provider`, `--xai-api-base-url`** — provider-specific;
  locode's provider registry + `--api-schema` already covers wire selection.
  Custom base-URL belongs to a provider's own config, not the core CLI.
- **`--max-budget-usd`** — needs a cost-tracking seam we don't have; defer until
  there's a cost meter. (Claude-only among the four.)
- **`--worktree`/`--tmux`, `--ide`, `--chrome`, `--share`, `--attach`,
  `--mdns`/network, teammate/swarm, remote/teleport** — orchestration/integration
  surface outside SPEC-TUI's core+TUI scope.
- **`--mcp-config`/`--strict-mcp-config`, `--plugin-dir`** — depend on MCP/plugin
  subsystems locode doesn't have; revisit if those land.
- **Grok's sticky `--minimal`/`--fullscreen` persistence** — adopt the *flag* if
  wanted, but skip the config-write stickiness (couples CLI to a side effect that
  `--bare` would then have to suppress).
- **`--input-format stream-json` / `--replay-user-messages`** — SDK-streaming-input
  surface; defer until locode grows a streaming-input mode. Our stdin path covers
  the common case.

### Naming guardrails for locode (from the clashes observed)

- Keep **`-p` = print** (never password, unlike opencode). Do **not** copy Grok's
  prompt-carrying `-p` — keep the boolean + positional shape.
- Use **distinct words** for the two "minimal" ideas: e.g. `--bare` for
  *skip-side-effects* and a separate TUI flag (`--inline`/`--no-alt-screen`) for
  *render mode*. Don't overload one word.
- Preserve compat **aliases** where cheap (`--yolo`, `--allowedTools`,
  `--dangerously-skip-permissions`) — Grok's precedent shows the muscle-memory
  payoff, and locode already aliases `--yolo`.

---

## Open questions

1. **Session persistence is the gating seam.** `-c/--continue`, `-r/--resume`,
   `--fork-session`, `--session-id` are the single highest-value cluster and all
   four harnesses expose them — but locode has no session store yet. Is a
   persistence + resume seam in scope, and does it warrant its own ADR before any
   of these flags land? (Fork semantics = Claude's mint-new-id default.)
2. **What exactly may a locode pack do at startup?** A `--bare` analog forces the
   question the Harness-fidelity-boundary memory already flags: which
   loop-adjacent/discovery behaviors (AGENTS.md walk, memory, future hooks) belong
   to a pack vs. the shared engine. `--bare`'s disable-list *is* that boundary —
   ADR-level.
3. **Permission model shape:** adopt Claude's single `--permission-mode`, or
   Codex's two orthogonal axes (`--ask-for-approval` × `--sandbox`)? The latter is
   more principled but implies a sandbox concept locode may not want in the core.
4. **`--model` vs. provider defaults:** does `--model` override the
   provider-registry default cleanly, or does the provider/wire coupling
   (`--api-schema`) constrain which models are valid? Needs the provider seam
   pinned before wording the flag.
5. **`--add-dir` and the jail:** how do extra dirs compose with the path-jail —
   additive writable roots (Codex) vs. tool-access allowlist (Claude)? Same word,
   different semantics; locode must pick one.
6. **Reasoning knob representation:** one shared `--reasoning-effort` string
   mapped per-wire, or a typed enum? Anthropic (thinking-token budget) and OpenAI
   (effort levels) don't share a scale.
7. **Config-file trajectory:** several skipped flags (`-c key=value`, settings,
   sticky render) only make sense once locode has a config file. Is that on the
   roadmap, and should the CLI reserve `-c` for continue (as today) and pick a
   different letter for config, or vice-versa?
