# Task 19 · Slice 3 — full gpt-5.6-sol prompt + `<environment_context>` preamble

> The codex pack's final slice: swap the minimal identity render for the full
> byte-exact gpt-5.6-sol `base_instructions` (D6), wire `strip_identity`, and add the
> `<environment_context>` User preamble item (D7). Completes the pack. Follows
> [`../../docs/codex-pack-dev-process.md`](../../docs/codex-pack-dev-process.md) (S3).
> Source: codex submodule `f201c30c` (2026-07-24).

## Phase 0 — status analysis
- **Merged (S1/S2):** `shell_command`, freeform `apply_patch`, the always-appended
  instructions block, the openai-responses-only wire requirement. Only the prompt is a
  stub (the minimal identity opener).
- **Next unit:** the real base prompt + the environment block — the last piece. No new
  tools, no new wire work.
- **Prereqs (hold):** `strip_identity` on `PackContext` ✓; the preamble already returns a
  `Vec<Message>` ✓.
- **Risks:** (1) byte-exact prompt fidelity (pinned + len/char/sha256 test); (2) the
  env-context *form* — the current codex is a per-turn world-state **diff** system, not the
  old static block; (3) timezone without a new dependency; (4) truth-vs-fidelity on the
  sandbox/permission portions of the env context.

## Phase 1 — source revisit (`f201c30c`, fresh citations)
- **Prompt** (`models-manager/models.json`, `models[0]`): slug `gpt-5.6-sol`,
  `apply_patch_tool_type=freeform`, `base_instructions` = **17730 chars / 17766 bytes**
  (multi-byte punctuation), sha256 `cbefa6b0…`. Opens with exactly
  `"You are Codex, an agent based on GPT-5. "` then `"You and the user share one
  workspace…"`. **Scanned for runtime placeholders / injected values: none** — no
  `{cwd}`-style templates, no model/version/date baked in (the only "GPT-5" is the identity
  opener). So it is reproduced **verbatim** (truth-vs-fidelity: nothing here is false for
  our run). Pinned in `prompts/base_instructions.md`.
- **`strip_identity`** removes exactly the identity opener sentence (the `IDENTITY`
  prefix). Default off (faithful); on = A/B contamination control.
- **`<environment_context>`** — the current codex renders it via a **world-state diff**
  (`context/world_state/environment.rs` `EnvironmentsState`, `WorldStateSection::render_diff`):
  a per-turn contextual **user** fragment that models codex's multi-environment + managed
  filesystem-permission + network-rule runtime state. For a single local environment with
  no exotic sandbox/network (the common case), the rendered form is exactly
  (`environment_render_tests.rs:60-80`):
  ```
  <environment_context>
    <cwd>{cwd}</cwd>
    <shell>{shell basename}</shell>
    <current_date>{date}</current_date>
    <timezone>{iana tz}</timezone>
  </environment_context>
  ```
  (2-space indent, `\n`-joined; `<shell>` is the basename, e.g. `bash`/`zsh`). `<filesystem>`
  and `<network>` are **omitted** in the default case (`filesystem: None`); XML text is
  escaped by `push_xml_escaped_text` (`&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`, `"`→`&quot;`,
  `'`→`&apos;`). `current_date`/`timezone` are `Option` (a read-only env still shows them).
- **Fidelity boundary + truth wins:** the **per-turn re-injection/diffing** of the env
  context is loop-adjacent machinery (ADR-0023, like Claude's `<system-reminder>`) → NOT
  reproduced; we emit the **static first-turn snapshot** as part of the preamble. The
  managed-sandbox `<permission_profile>`/`<filesystem>`/`<network>` XML models runtime state
  we don't carry (we use a path jail) → reproducing it would inject **untrue** values → we
  omit it (matches codex's own default-case omission).

## Design
- **`prompt.rs`:** `BASE_INSTRUCTIONS = include_str!("prompts/base_instructions.md")`;
  `render(ctx)` = the full prompt, minus the `IDENTITY` prefix when `strip_identity`.
  `environment_context(ctx)` renders the `<cwd>/<shell>/<current_date>/<timezone>` block
  (tz line only when `ctx.timezone` is `Some`), XML-escaped, shell → basename.
- **`PackContext`:** add `timezone: Option<String>` (codex-only; follows the
  `is_git_repo`/`model`/`os_version` precedent). The exec/tui layers resolve it
  **dependency-free** (`$TZ`, else the `/etc/localtime` symlink target after `zoneinfo/`);
  `None` omits the line. (`iana-time-zone` is already transitive via chrono, but adding a
  *direct* dep is an ask-first item — avoided.)
- **`preamble()`** = `[System(render(ctx) + "\n" + APPLY_PATCH_INSTRUCTIONS),
  User(environment_context(ctx))]` (D7).

## Gap log (this slice)
- **Env-context is the static first-turn snapshot only** — codex re-injects/diffs it per
  turn (loop-adjacent machinery, ADR-0023), which stays on the shared engine.
- **`<filesystem>`/`<network>`/`<permission_profile>` omitted** — codex's managed-sandbox
  and network-rule model is runtime state we don't carry; reproducing it would be untrue
  (truth wins). Codex itself omits `<filesystem>` in the default case.
- **Timezone is best-effort, dep-free** — real IANA name when resolvable, else omitted
  (codex's field is optional). Not codex's tz-detection library (would be a new dep).

## Test matrix
- Prompt pin: `render` byte len 17766 + char count 17730 + starts-with identity +
  contains `# Personality` (sha256 in a comment).
- `strip_identity`: output starts with `"You and the user share one workspace"` and is
  exactly `17766 - IDENTITY.len()` bytes.
- `environment_context`: exact string for cwd/shell(basename)/date/tz; tz omitted when
  `None`.
- Preamble: `[System, User]`; System starts with the identity, contains `# Personality`
  AND the appended `## `apply_patch`` block; User starts with `<environment_context>` and
  carries `<shell>zsh</shell>` + the tz line.
- Existing S1/S2 codex tests still green; all `PackContext` literals updated for the new
  field.

## Preset target
`locode -p --harness codex --api-schema mock "hi"` → `harness:"codex"`; four-part gate
green; the `stream-json` init shows System(full prompt + apply_patch) + User(env context)
+ tools `apply_patch`(freeform)/`shell_command`(json_schema).

## Result

**Shipped** (2026-07-24, branch `feat/task-19-slice-3-prompt`). **The codex pack is
complete.**

- **`prompt.rs`** now serves the full pinned gpt-5.6-sol `base_instructions` (17766 B /
  17730 chars, sha256 `cbefa6b0…`, `prompts/base_instructions.md`) with `strip_identity`
  removing the identity opener, plus the `environment_context()` renderer.
- **`PackContext.timezone: Option<String>`** added; the exec (`run.rs::timezone`) and tui
  (`engine.rs::detect_timezone`) layers resolve it dependency-free (`$TZ` /
  `/etc/localtime`). All 13 `PackContext` construction sites updated.
- **Preamble** = `[System(full prompt + apply_patch instructions), User(<environment_context>)]`
  (D7). Verified via the `stream-json` init: System 20801 B starting with the identity and
  containing the apply_patch block; User `<environment_context>` with the real cwd/shell/
  date + machine timezone.
- **Tests:** +3 net codex tests (full-prompt pin, strip-identity-only-opener, env-context
  render + tz-omitted); 35 codex tests total. Full four-part gate green; preset →
  `"harness":"codex"`.
- The pack ships `shell_command` + freeform `apply_patch` + the full gpt-5.6-sol prompt,
  openai-responses-only. Deferred (unchanged): `update_plan`, unified exec / background,
  the per-turn env-context diffing, subagents/skills/MCP/compaction (shared engine /
  ADR-0023 fidelity boundary).