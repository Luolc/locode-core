# Per-task implementation plans

Detailed, **source-grounded** pre-implementation plans for the remaining v0 tasks
(6–14), one file per task. Each was written by re-reading the actual harness source
in the `coding-cli-survey` submodules (per the AGENTS.md "read the source before
planning" rule) and cites concrete `file:line`. They expand [`../todo.md`](../todo.md)
from a checklist into a reviewable design.

> **Status: drafts for review.** These capture recommendations and a set of *open
> questions* that need sign-off before implementation. They are not the final word —
> decisions get confirmed with the user, then folded into the task as it is built.
> Where a plan predates a merged crate, its "open questions" may already be resolved
> (see below).

| Plan | Task |
|---|---|
| [task-06-engine-loop.md](task-06-engine-loop.md) | `locode-engine` — the sample→dispatch→append loop + `Session` |
| [task-07-host.md](task-07-host.md) | `locode-host` — path jail, shell exec (timeout/caps), truncation |
| [task-08-packs.md](task-08-packs.md) | `locode-packs` — pack framework + grok pack wiring |
| [task-09-grok-read-terminal.md](task-09-grok-read-terminal.md) | grok `run_terminal_cmd` + `read_file` |
| [task-10-grok-edit.md](task-10-grok-edit.md) | grok `write` + `search_replace` (edit invariants) |
| [task-11-grok-search.md](task-11-grok-search.md) | grok `grep` + `glob` (ripgrep-backed) |
| [task-12-anthropic-wire.md](task-12-anthropic-wire.md) | Anthropic Messages wire (the live `Provider`) |
| [task-13-grok-prompt.md](task-13-grok-prompt.md) | grok pack system prompt (minijinja) |
| [task-14-facade-exec.md](task-14-facade-exec.md) | `locode` facade + `locode-exec` binary |

## Already resolved since these were written

- **`Completion` / `StopReason` / `ConversationRequest` shapes** — settled and shipped
  in Task 5 (`Completion` carries `Vec<ContentBlock>`; no `system` field on the
  request; `StopReason` is `#[non_exhaustive]` + `Unknown(String)`). Some plans list
  these as open — they are not.

## Cross-cutting decisions still open (flagged by multiple plans)

- **`run_terminal_cmd`** is grok's real tool name (SPEC/todo say `run_terminal_command`).
- **Grok has no standalone `write` tool** — creation is `search_replace` with empty
  `old_string`; a dedicated `write` is a documented locode/OpenCode-sourced addition.
- **Edit invariants #1 (read-before-edit) and #3 (mtime freshness) are a locode
  addition**, not grok-faithful (grok's `search_replace` hard-codes freshness off);
  #2 (exact+unique) and #4 (reject no-op) are ported verbatim.
- **`repair_pairing` home** — proposed for `locode-provider` (the provider layer, per
  ADR-0004) rather than `locode-protocol`.
- **`--provider` → `--api-schema`** rename (report field + `Event::Init` + CLI flag)
  for consistency with `Provider::api_schema()`.
