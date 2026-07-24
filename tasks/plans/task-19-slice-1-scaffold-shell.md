# Task 19 · Slice 1 — pack scaffold + `shell_command` + minimal prompt

> Faithful port of codex's `shell_command` (non-PTY shell) + a minimal-but-real
> prompt, wiring `--harness codex`. Follows
> [`../../docs/codex-pack-dev-process.md`](../../docs/codex-pack-dev-process.md) (S1).
> Source: codex submodule `f201c30c` (2026-07-24).

## Phase 0 — status analysis
- **Merged:** grok + claude packs (the template); `Pack::shape_user_prompt`,
  `PackContext.{is_git_repo,model,os_version}`, `Host::create_dir` all exist.
- **Next unit:** `CodexPack` + `Pack` impl + `shell_command` (simplest tool, no
  shared state, no apply_patch/wire complexity) + a minimal prompt (identity opener) +
  `--harness codex` through exec/tui. Proves the pack end-to-end on `--api-schema mock`.
- **Why now / unblocks:** shell_command has no dependency on apply_patch or the
  responses-only wire requirement (S2). Establishes the module layout + the codex
  output framing.
- **Prereqs (hold):** `Host::exec` seam ✓; identity-branch prompt pattern ✓.
- **Risks:** (1) the exact output framing bytes (confirmed below); (2) codex has no
  timeout clamp but our host clamps at 10 min (documented); (3) combined vs interleaved
  output ordering (documented gap, same as claude Bash).

## Phase 1 — source revisit (`f201c30c`, fresh citations)
- **Schema** (`core/src/tools/handlers/shell_spec.rs` `create_shell_command_tool`):
  name `shell_command`; `strict:false`; `additionalProperties:false`; required
  `["command"]`; `output_schema:None`. Params (non-Windows): `command` ("Shell script
  to run in the user's default shell."), `workdir` ("Working directory for the command.
  Defaults to the turn cwd."), `timeout_ms` ("Maximum command runtime. Defaults to 10000
  ms."), `login` ("True runs with login shell semantics; false disables them. Defaults
  to true."). **Approval params dropped (D8):** `sandbox_permissions`/`justification`/
  `prefix_rule` (no interactive permission flow — top gap).
- **Description** (non-Windows branch, verbatim): `"Runs a shell command and returns
  its output.\n- Always set the \`workdir\` param when using the shell_command function.
  Do not use \`cd\` unless absolutely necessary."` Pinned in `descriptions/shell_command.md`.
- **Timeout** (`core/src/exec.rs:58` `DEFAULT_EXEC_COMMAND_TIMEOUT_MS=10_000`;
  `handlers/shell/shell_command.rs:113` `timeout_ms.into()`): default 10000 ms, **no max
  clamp**. Kill on timeout (`exec.rs:1016`), exit code **124** (`EXEC_TIMEOUT_EXIT_CODE`).
- **Model-facing output framing** (`core/src/tools/mod.rs:78-100`
  `format_exec_output_for_model` — confirmed the model path via `events.rs:375`; the
  `_str` variant is post-tool-use-hook only):
  ```
  Exit code: {exit_code}
  Wall time: {duration_seconds} seconds        // round(secs_f32*10)/10
  Total output lines: {total_lines}            // ONLY when truncation occurred
  Output:
  {truncate_middle(content)}
  ```
  joined by `\n`. `content` = timeout ? `"command timed out after {ms} milliseconds\n{aggregated}"` : `aggregated` (`build_content_with_timeout`, `mod.rs:116`).
- **Truncation** (gpt-5.6-sol `truncation_policy = {tokens: 10000}`,
  `models.json`): middle truncation at a 10000-token budget;
  `APPROX_BYTES_PER_TOKEN=4` (`utils/string/src/truncate.rs:4`) → ~40000-byte budget;
  marker `…{removed} tokens truncated…` (`format_truncation_marker`). We approximate
  with a byte-safe middle truncation (D9).
- **Shell shape:** codex runs in the user's default shell with login semantics
  (`login` default true) → `ShellSpec { detect_program:true, login_arg:<login>,
  login_path_probe:false }`.

## Design
```
crates/locode-packs/src/codex/
├── mod.rs                    # CodexPack + Pack impl + register(shell_command) + preamble + tests
├── prompt.rs                # minimal render (gpt-5.6-sol identity opener + strip_identity); S3 → full
├── shell_command.rs         # the tool (framing + middle-truncation)
└── descriptions/shell_command.md
```
- **Args** (`deny_unknown_fields`): `command: String`; `workdir: Option<String>`;
  `timeout_ms: Option<u64>`; `login: Option<bool>`. Approval params NOT present (D8).
  Type-strict.
- **run():** resolve `workdir` (or cwd) in jail; `Host::exec` (ShellSpec login per
  `login`, default true; timeout default 10000, no clamp beyond the host's 10-min cap —
  documented); render the framing; middle-truncate the combined output; a non-zero exit
  / timeout is a **successful capture** (Ok, exit 124 in the text — codex sets
  `success:true`), only a spawn failure is a soft error.
- **Minimal prompt** (S1): the gpt-5.6-sol identity opener sentence(s); `strip_identity`
  removes "You are Codex, an agent based on GPT-5. ". Full 17730-byte prompt lands in S3.
- **preamble()** = `[System(render(ctx))]` (S1; S3 adds the `<environment_context>` User item).
- **Wiring:** `lib.rs` (CodexPack, PACKS); `Harness::Codex` (exec/cli.rs) + `as_str`
  "codex". `shape_user_prompt` = default (verbatim). The responses-only wire requirement
  lands in S2 (with apply_patch).

## Gap log (this slice)
- **Approval/sandbox params dropped** (D8) — top faithfulness gap.
- **No timeout clamp in codex**, but the host caps at 10 min — a host-safety divergence.
- **Combined output = stdout then stderr concatenated**, not real-time interleaved (host
  limitation; same as claude Bash).
- **Truncation** approximates codex's `truncate_middle_with_token_budget` (byte-safe
  middle truncation at 10k-token≈40k-byte budget).

## Test matrix
- Schema golden: `shell_command` — `additionalProperties:false`; `command`/`workdir`/
  `timeout_ms`/`login` present with verbatim descriptions; approval params ABSENT;
  only `command` required.
- Description provenance pin (`shell_command.md` len + sha256 + opener).
- Prompt: minimal render starts with the identity; `strip_identity` removes it.
- Behavior (`build_registry`+`dispatch`, tempdir host): echo → `Exit code: 0\nWall time:
  … seconds\nOutput:\nhi`; non-zero exit → soft-ok with `Exit code: N`; timeout →
  `Exit code: 124` + "command timed out after …"; large output middle-truncated with the
  marker + `Total output lines:`.
- Pack: `resolve("codex")`; `available()` lists grok+claude+codex; specs = `["shell_command"]`.

## Preset target
`locode -p --harness codex --api-schema mock "hi"` → Report with `harness:"codex"`;
four-part gate green.

## Result

**Shipped** (2026-07-24, branch `feat/task-19-slice-1-scaffold-shell`). The codex
pack scaffold + `shell_command` + a minimal identity prompt are wired end-to-end.

- **Files:** `crates/locode-packs/src/codex/{mod.rs, prompt.rs, shell_command.rs,
  descriptions/shell_command.md}`; wired `lib.rs` (`CodexPack`, `PACKS`, resolver
  tests) + `Harness::Codex` in `crates/locode-exec/src/cli.rs`.
- **`shell_command`** ports the non-Windows branch: framing `Exit code:` / `Wall
  time:` / (`Total output lines:` only on truncation) / `Output:`; byte-safe middle
  truncation at a 10k-token≈40k-byte budget with the `…{N} tokens truncated…` marker;
  default 10000 ms timeout with no max clamp (host's 10-min ceiling still applies);
  timeout → exit **124** with the `command timed out after …` prefix; non-zero exit is
  a successful capture (Ok). Args `deny_unknown_fields`, type-strict; approval/sandbox
  params dropped (D8).
- **Description** pinned in `descriptions/shell_command.md` (161 bytes, sha256
  `b3ee7fb0…`) with a byte-length + digest test.
- **Prompt** = minimal identity render; `strip_identity` removes `"You are Codex, an
  agent based on GPT-5. "`. Full 17730-byte gpt-5.6-sol prompt is S3.
- **Tests:** 12 codex tests pass (schema golden with approval params absent, echo
  framing, non-zero-exit soft-ok, timeout→124, unknown-field rejection, prompt render +
  strip, description pin, registration, single-System preamble). Full workspace green.
- **Gate:** `fmt · clippy · test · doc` all clean; preset
  `locode -p --harness codex --api-schema mock "hi"` → `"harness":"codex"`.
- **Deferred to later slices (unchanged):** apply_patch (S2), openai-responses-only
  wire enforcement (S2), full gpt-5.6-sol prompt + `<environment_context>` preamble (S3).
