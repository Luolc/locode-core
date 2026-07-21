# Task 28 — `locode -p` headless mode (unify the binary; retire-exec path)

Per [`docs/tui-dev-process.md`](../../docs/tui-dev-process.md) (autonomous loop;
this is a user-requested feature). Grounding: survey `00-overview/
headless-mode.md`; ADR-0001/0009/0019.

## Phase 0 — status analysis

- **State**: `locode` (locode-app→locode-tui) is interactive-only; `locode-exec`
  is the headless one-shot binary. User wants `locode -p "…"` to run headless
  (Claude-Code style), so the installer can later ship `locode` and retire
  `locode-exec`.
- **Minimal next unit**: add a `-p`/`--print` mode to the `locode` binary that
  runs the headless one-shot (reusing locode-exec's proven engine), leaving
  the default (no `-p`) as the TUI.
- **Why now**: user request; the TUI is feature-complete, so unifying the
  binary is the next step toward a single installable `locode`.
- **Prereqs**: locode-exec's headless run (exists, tested); locode-tui's TUI
  entry (exists).
- **Unblocks**: retiring `locode-exec`; installer shipping `locode`.
- **Risks**: (1) two clap CLIs on one argv — avoid by a single unified CLI in
  locode-tui that dispatches; (2) stdout discipline — headless prints must
  stay inside locode-exec's audited writers; (3) harness enum vs string.

## Phase 1 — harness revisit (fresh reads 2026-07-21)

- **Claude Code** (`headless-mode.md`, re-read): `claude -p "…"` / `--print`;
  `main.tsx` detects `-p` → `print.ts runHeadless`; headless is the SAME loop
  with the UI removed + an output emitter swapped in; trust/permission UI
  skipped, auto-allow. → **Adopt exactly**: `-p` selects headless; the default
  is the TUI; the dispatch lives in the binary's entry (our `main_with`).
- **grok** `-p` → `xai-grok-pager/src/headless.rs run_single_turn`; **codex**
  `codex exec "…"` (subcommand). → We follow Claude/grok's `-p` flag over
  codex's subcommand (the user asked for `-p`).
- Permissions headless: all four auto-allow/bypass. → Our headless already
  uses the default `AllowAll` approver (locode-exec builds no `TuiApprover`);
  `-p` inherits that. `--yolo` in headless still just lifts the path jail.

**Decisions**: `-p`/`--print` on the unified `locode` CLI selects headless;
prompt is positional (or stdin), matching locode-exec. The headless run reuses
locode-exec's `run` (session assembly + `--output-format` emit + SIGTERM),
exposed as `run_headless(cli, registry)`. locode-tui depends on locode-exec
(lib) for now; when locode-exec retires, that logic migrates into locode-tui/a
shared lib (recorded in the ADR). In TUI mode a positional prompt pre-fills the
composer (nice-to-have, low-risk).

## Phase 2 — design

- **locode-exec**: extract `pub fn run_headless(cli: cli::Cli, registry:
  ProviderRegistry) -> ExitCode` (everything `main_with` does after
  `Cli::parse`); `main_with` = parse + `run_headless`. Re-export `cli::{Cli,
  Harness, OutputFormat}` at the crate root. Behavior unchanged externally.
- **locode-tui**: add `locode-exec` dep. Unify `cli::Cli`:
  - reuse `locode_exec::Harness` (ValueEnum) for `--harness` and
    `locode_exec::OutputFormat` for `--output-format`;
  - add `-p`/`--print` (bool), positional `prompt: Option<String>`,
    `--max-turns`.
  - `to_headless(self) -> locode_exec::Cli` maps the shared fields.
  - `main_with`: `if cli.print { return locode_exec::run_headless(
    cli.to_headless(), registry); }` else the TUI path.
  - engine.rs: harness is now the enum → `harness.as_str()`.
  - App/composer: an optional initial draft from the positional prompt.
- **Docs (ADR-first)**: ADR-0019 dated amendment (the `locode` binary is the
  unified entry: TUI default + `-p` headless; dispatch in `main_with`;
  locode-exec reused now, to be retired; installer to ship `locode`).
  SPEC.md assumption 1 reconciled (locode-exec is no longer the *only*
  headless surface). todo.md Task 28.

### Edge cases

`-p` with no prompt + no stdin → locode-exec's empty-prompt error (exit 1);
`-p --output-format stream-json` (headless JSONL); bare `locode` with a
positional prompt (pre-fills composer, doesn't auto-send); `--harness`
unknown (clap rejects — closed enum now); `-p` + `--yolo` (jail lifted,
still auto-allow); SIGTERM under `-p` (locode-exec's handler, unchanged).

### Test matrix / preset targets

1. [exec] existing locode-exec integration tests pass unchanged (the
   main_with→run_headless refactor is behavior-preserving).
2. [tui integration] `Cli { print: true, prompt: Some("hi"), api_schema:
   "mock", … }.to_headless()` → `run_headless` → exit 0 (drive the mock; or a
   process-level test via the `locode` binary).
3. [tui reducer] a positional prompt pre-fills the composer (App::with_draft).
4. [PTY/process smoke] `locode -p "say hi" --api-schema mock` → one JSON
   report line on stdout, exit 0; `locode -p … --output-format text` → final
   message; bare `locode --api-schema mock` → TUI (draws).
5. [gates] fmt/clippy/test/doc green (FAILED-explicit check).

## Open questions for the user (non-blocking)

- Retiring `locode-exec` + switching the installer to `locode` is a **later**
  step (user said "after this version") — not done here; flagged for the
  release decision.

## Result (2026-07-21)

Shipped: `locode_exec::run_headless(cli, registry)` extracted from `main_with`
(behavior-preserving; `Cli`/`Harness`/`OutputFormat` re-exported); unified
`locode-tui::cli::Cli` (`-p`/`--print`, positional prompt, `--output-format`,
`--max-turns`, sharing exec's `Harness`/`OutputFormat`); `main_with`
print-dispatch (`-p` → `run_headless`, else TUI); positional prompt pre-fills
the composer in TUI mode (`App::with_draft`); ADR-0019 amendment + SPEC
reconcile + todo Task 28.

All preset targets met: 346 workspace tests — 3 new `-p` process integration
tests (json one-line / text final-message / unknown-schema pre-run fail),
`with_draft` reducer test; locode-exec's own integration suite unchanged. Full
gates + doc green (FAILED-explicit check). Release-binary smokes: `locode -p
"say hi" --api-schema mock` → one JSON report (exit 0); `--output-format text`
→ final message; `stream-json` → `init`…`result`; `--help` lists `-p/--print`;
bare `locode "task"` launches the TUI with the composer pre-filled.

Deviation: none. **Retire plan (user-gated, not done):** drop the
`locode-exec` binary + switch installers to `locode` after this version; the
headless logic then migrates out of locode-exec and the crate edge is dropped.
