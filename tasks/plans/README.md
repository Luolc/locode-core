# Per-task implementation plans

Detailed, **source-grounded** design records — one file per task. Each was written by
re-reading the actual harness source in the `coding-cli-survey` submodules (per the
AGENTS.md "read the source before planning" rule) and cites concrete `file:line`.

**These are point-in-time records, not status trackers.** A plan captures the design as of
when it was written (and, for shipped tasks, often a "Result" addendum); it may say
"planning" in its header even though the task has since shipped. **Live task status —
what is done, in progress, or next — lives only in [`../tracker.md`](../tracker.md).**

## Index

Grouped as in the tracker. Status column intentionally omitted (see above) — check the
tracker.

### v0 core spine
| Plan | Task |
|---|---|
| [task-01-workspace-scaffold.md](task-01-workspace-scaffold.md) | Cargo workspace + crate boundaries + toolchain/lints |
| [task-03-protocol-conversation-report.md](task-03-protocol-conversation-report.md) | `locode-protocol` — conversation model + report envelope |
| [task-03b-streaming-events.md](task-03b-streaming-events.md) | `locode-protocol` — `stream-json` events + reconstruction |
| [task-04-tools-contract-registry.md](task-04-tools-contract-registry.md) | `locode-tools` — `Tool` contract + registry + dispatch door |
| [task-05-provider-mock.md](task-05-provider-mock.md) | `locode-provider` — trait + `Completion` + mock + repair |
| [task-06-engine-loop.md](task-06-engine-loop.md) | `locode-engine` — the sample→dispatch→append loop + `Session` |

### grok harness pack + host seam
| Plan | Task |
|---|---|
| [task-07-host.md](task-07-host.md) | `locode-host` — path jail, shell exec (timeout/caps), truncation |
| [task-08-packs.md](task-08-packs.md) | `locode-packs` — pack framework + grok pack wiring |
| [task-09-grok-read-terminal.md](task-09-grok-read-terminal.md) | grok `run_terminal_cmd` + `read_file` |
| [task-10-grok-edit.md](task-10-grok-edit.md) | grok `search_replace` (edit invariants; no standalone `write`) |
| [task-11-grok-search.md](task-11-grok-search.md) | grok `grep` (ripgrep) + `list_dir` (grok's fs walker) |

### Live wires + facade
| Plan | Task |
|---|---|
| [task-12-anthropic-wire.md](task-12-anthropic-wire.md) | Anthropic Messages wire (the live `Provider`) |
| [task-13-grok-prompt.md](task-13-grok-prompt.md) | grok pack system prompt (MiniJinja) |
| [task-14-facade-exec.md](task-14-facade-exec.md) | `locode` facade + `locode-exec` binary |
| [task-18-openai-responses-wire.md](task-18-openai-responses-wire.md) | OpenAI Responses wire (`openai-responses`; stateless, freeform tools, encrypted-reasoning replay, transport hoist) |
| [task-17-openai-chat-wire.md](task-17-openai-chat-wire.md) | OpenAI Chat Completions wire (`openai-chat`; LCD/control wire) |

### More harness packs
| Plan | Task |
|---|---|
| [task-19-codex-pack.md](task-19-codex-pack.md) | codex pack (`shell_command` + freeform `apply_patch` + prompt; `update_plan` not ported — see the dev-process doc) |
| [task-19-slice-1-scaffold-shell.md](task-19-slice-1-scaffold-shell.md) | codex pack slice 1 — pack scaffold + `shell_command` + minimal prompt |
| [task-19-slice-2-apply-patch.md](task-19-slice-2-apply-patch.md) | codex pack slice 2 — freeform `apply_patch` + instructions block + responses-only wire |
| [task-19-slice-3-prompt.md](task-19-slice-3-prompt.md) | codex pack slice 3 — full gpt-5.6-sol prompt + `<environment_context>` preamble (pack complete) |
| [task-15-opencode-pack.md](task-15-opencode-pack.md) | opencode pack (bash/read/write/edit/glob/grep/apply_patch + per-provider prompt) |
| [task-20-claude-pack.md](task-20-claude-pack.md) | claude pack (Bash/Read/Edit/Write/Glob/Grep + freshness gate + prompt) |
| [task-20-slice-1-scaffold-bash.md](task-20-slice-1-scaffold-bash.md) | claude pack slice 1 — pack scaffold + Bash + minimal prompt |
| [task-20-slice-2-read-freshness.md](task-20-slice-2-read-freshness.md) | claude pack slice 2 — Read + ClaudeSessionState freshness store |
| [task-20-slice-3-edit.md](task-20-slice-3-edit.md) | claude pack slice 3 — Edit (read-before-edit + staleness gate) |
| [task-20-slice-4-write.md](task-20-slice-4-write.md) | claude pack slice 4 — Write (create-or-overwrite, read-before-write gate) |
| [task-20-slice-5-glob.md](task-20-slice-5-glob.md) | claude pack slice 5 — Glob (rg --files, mtime sort, cap 100) |
| [task-20-slice-6-grep.md](task-20-slice-6-grep.md) | claude pack slice 6 — Grep (full ripgrep passthrough, 3 output modes) |
| [task-20-slice-7-prompt.md](task-20-slice-7-prompt.md) | claude pack slice 7 — full system prompt + env + preamble (pack complete) |

### TUI core prerequisites + TUI + streaming
| Plan | Task |
|---|---|
| [task-23-25-tui-core-prereqs.md](task-23-25-tui-core-prereqs.md) | Session continuity (ADR-0016) + cancellation (ADR-0018) + approval seam (ADR-0017) |
| [task-27-slice-1-shell.md](task-27-slice-1-shell.md) | TUI slice 1 — shell: crates, terminal lifecycle, event loop, composer |
| [task-27-slice-2-drive-a-run.md](task-27-slice-2-drive-a-run.md) | TUI slice 2 — drive a run (mock) |
| [task-27-slice-3-cancel.md](task-27-slice-3-cancel.md) | TUI slice 3 — cancel |
| [task-27-slice-4-approvals.md](task-27-slice-4-approvals.md) | TUI slice 4 — approvals |
| [task-27-slice-5-polish.md](task-27-slice-5-polish.md) | TUI slice 5 — conversation polish |
| [task-27-slice-6-hardening.md](task-27-slice-6-hardening.md) | TUI slice 6 — hardening / release |
| [task-27-slice-7-markdown-fixes.md](task-27-slice-7-markdown-fixes.md) | TUI slice 7 — markdown fixes |
| [task-27-slice-8-composer-status.md](task-27-slice-8-composer-status.md) | TUI slice 8 — composer + status bar |
| [task-27-slice-9-code-highlighting.md](task-27-slice-9-code-highlighting.md) | TUI slice 9 — code highlighting (ADR-0020) |
| [task-28-unified-p-headless.md](task-28-unified-p-headless.md) | unified `locode` binary — `-p` headless mode |
| [task-29-live-streaming.md](task-29-live-streaming.md) | live token streaming (ADR-0021) |

### Shared context machinery
| Plan | Task |
|---|---|
| [task-30-agents-md-project-instructions.md](task-30-agents-md-project-instructions.md) | Task 30 — shared `AGENTS.md` project-instruction loading (ADR-0023) |
