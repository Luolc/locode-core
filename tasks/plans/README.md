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

## Cross-cutting decisions — RESOLVED

- **`repair_pairing` home → `locode-provider`.** Provider-layer concern (ADR-0004);
  the engine depends on provider, so it calls it each iteration. Landed in Task 6.
- **`provider` → `api_schema` rename → done** in the report envelope, `Event::Init`,
  ADR-0009, and the golden snapshot (Task 6). The CLI flag becomes `--api-schema` in
  Task 14. It names the wire *schema*, not a gateway.
- **`run_terminal_cmd`** — use grok's real name (the SPEC/todo `run_terminal_command`
  was a voice-input artifact, not a real discrepancy). Applies at Task 9.
- **Standalone `write` tool → skip in the grok pack.** Grok creates files via
  `search_replace` with empty `old_string`; a dedicated `write` is not grok's. Revisit
  when implementing other harness packs. Applies at Task 10.
- **Faithfully mimic Grok Build** for the grok pack tools' behavior/details — this
  governs the edit-invariant question (implement grok's real `search_replace`
  semantics). Applies at Tasks 9–11; confirm the exact #1/#3 treatment at Task 10.
