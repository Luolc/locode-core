# Task 15 — `locode-packs` opencode pack (faithful port of opencode's tools + prompt)

> Implementation plan, written **before** code. Faithful mimicry per AGENTS.md: the
> pack reproduces opencode's **real tools** (names, arg schemas, verbatim
> descriptions, caps, guardrails) and its **static system prompt + static preamble**
> — and nothing loop-adjacent (the fidelity boundary, ADR-0023: reminder machinery,
> TodoWrite/plan loops, subagent orchestration, compaction stay on the shared
> engine). Cites the opencode source under
> `~/dev/coding-cli-survey/submodules/opencode` (submodule commit `17544802c`, read
> 2026-07-24 via the tool survey) as `file:line`, plus `survey/04-opencode/*`.
> Repo precedent: the grok (Tasks 9–13) and **claude (Task 20)** packs are the
> pattern for tool ports, `descriptions/` provenance pins, byte-pin snapshots, the
> `strip_identity` knob, `Pack::shape_user_prompt`, the `is_git_repo`/`model`/
> `os_version` `PackContext` fields, and `Host::create_dir` for mkdir-on-create.
>
> **Scope note — Task 15 has two halves.** This plan covers the **`opencode` faithful
> port**. Our own **`locode` best-of pack** (grok-build-style naming; ADR-0011's
> rg-glob is scoped there) is the *other* half of Task 15 and gets its own plan when
> scheduled — it is explicitly NOT a faithful port and its decisions (which harness's
> best idea to adopt per tool) are independent.

---

## Current-scope reconciliation (2026-07-24) — read alongside the claude precedent

This plan is written **after** the claude pack, so it already assumes everything
that lagged in the older codex plan (Task 19):
- **Fidelity boundary = ADR-0023** (not "AGENTS.md" loosely). Project-instruction
  loading (opencode's AGENTS.md-style custom instructions) comes from the **shared
  engine loader**, not the pack — the pack does NOT load or inject them (like claude
  D11). The shared loader uses one injection format for every pack, so opencode's
  exact custom-instructions wrapper is a documented fidelity gap, not a pack feature.
- **`Pack::shape_user_prompt`** exists (default = verbatim). opencode sends the raw
  user prompt (no `<user_query>`-style wrapper) → uses the default.
- **`PackContext`** already carries `cwd/os/shell/date/headless/strip_identity` +
  `is_git_repo/model/os_version` — enough for opencode's dynamic env block.
- **`Host::create_dir`** (parents + exist_ok) exists — `write`/`edit`/`apply_patch`
  create parent dirs on create the way opencode does, without touching `write_file`.
- **Truth-first (AGENTS.md "Fidelity vs. truth"):** where opencode injects something
  untrue for our run (its prompts self-identify as "OpenCode"; the env block names a
  model/provider), reproduce faithfully **unless** it would be an actual lie — prefer
  the harness's own off/absent branch, drop low-eval-impact runtime-untrue lines.
- **Descriptions:** opencode stores tool descriptions as `.txt` files imported via
  `import DESCRIPTION from "./x.txt"` — **this is exactly our `include_str!` model**
  (opencode is the pure-static end of the description-interface spectrum,
  `docs/research/tool-description-interface.md`). We port them verbatim into
  `descriptions/*.md`, provenance-pinned. Two exceptions are dynamic — see §9 Q1/Q3.

---

## 1. Purpose & scope

Port opencode's headless-relevant toolset and base prompt as `--harness opencode`,
the fourth studied-harness pack (after grok/claude; codex in flight). opencode is the
**"more tools, fuzzier edits"** point in the A/B space: a full fs/search toolset like
claude's, **plus** a distinct `apply_patch` (codex-style envelope) that is
*model-gated against* `edit`/`write`, and an `edit` tool with a **9-strategy fuzzy
matcher** — the opposite philosophy from grok's/claude's exact-string edits and from
our own type-strict policy. That fuzziness is opencode's real behavior and the A/B
signal; we reproduce it faithfully.

### 1.1 Full tool inventory (registry `packages/opencode/src/tool/registry.ts:224-244`)

| Wire id | Source | Headless verdict |
|---|---|---|
| `bash` | `shell.ts` (id `bash`, not `shell` — `shell/id.ts:14-16`) | **port** (Shell) |
| `read` | `read.ts` | **port** (Read) — also does directory listing (no separate `ls`) |
| `write` | `write.ts` | **port** (Write) |
| `edit` | `edit.ts` | **port** (Edit) — 9-strategy fuzzy matcher |
| `glob` | `glob.ts` | **port** (Glob) — ripgrep `--files` |
| `grep` | `grep.ts` | **port** (Grep) — ripgrep `--json` |
| `apply_patch` | `apply_patch.ts` | **port** (Patch) — codex-style envelope; model-gated vs edit/write (§1.2) |
| `webfetch` | `webfetch.ts` | **defer** — network (no HTTP host seam); schema/desc notable for later |
| `todowrite` | `todo.ts` | **exclude** — session todo loop (loop-adjacent) |
| `task` | `task.ts` | **exclude** — subagent orchestration; description is runtime-built from the agent registry |
| `skill` | `skill.ts` | **exclude** — loads skill markdown into context (loop-adjacent) |
| `plan_exit` | `plan.ts` | **exclude** — plan-mode switch (interactive, flag+cli only) |
| `question` | `question.ts` | **exclude** — interactive Q&A (`requiresUserInteraction`) |
| `websearch` | `websearch.ts` | **exclude/defer** — Exa/Parallel, provider-gated |
| `execute` (code-mode) | `code-mode.ts` | **exclude** — experimental sandboxed JS interpreter over MCP |
| `lsp` | `lsp.ts` | **exclude** — experimental LSP tool (flag-gated) |
| `invalid` | `invalid.ts` | **exclude** — engine sentinel for malformed calls ("Do not use") |

**Headless subset = { `bash`, `read`, `write`, `edit`, `glob`, `grep`, `apply_patch` }
— 7 tools** (webfetch deferred as an 8th when a fetch host seam lands). ToolKinds:
`Shell`, `Read`, `Write`, `Edit`, `Glob`, `Grep`, + a new `Patch`-ish kind for
apply_patch (align with the codex pack's apply_patch kind — coordinate).

### 1.2 The model-gated apply_patch ⟷ edit/write visibility (an open question — §9 Q2)

opencode does **not** always expose all seven. `registry.ts:286-298`: `usePatch =
modelID.includes("gpt-") && !includes("oss") && !includes("gpt-4")`. When `usePatch`
→ only `apply_patch` is visible (edit + write hidden). Otherwise → `edit` + `write`
visible, `apply_patch` hidden. A **substring gate on the model id**, not a capability
flag. Since `PackContext.model` is available, we *can* reproduce this at register-time
— but it means the tool surface varies per run. See §9 Q2 for the decision.

### 1.3 Deferred (reserved seams)
`webfetch` (fetch host seam) · `websearch` · `task`/subagents · `todowrite` · `skill`
· `code-mode`/`lsp` · opencode's tree-sitter bash permission analysis (engine/approval
seam) · LSP-diagnostics tails on read/write/edit (engine seam) · the shared
`Truncate` spill-to-file service (we keep the 2000-line/50 KB caps, not the file spill).

---

## 2. Module layout
```
crates/locode-packs/src/opencode/
├── mod.rs           # OpencodePack + Pack impl + register + tests
├── prompt.rs        # per-provider base prompt selection + dynamic env block + preamble
├── bash.rs          # bash (dynamic desc → rendered unix profile, §9 Q1)
├── read.rs          # read (file + directory branch; N: line format; caps)
├── write.rs         # write (read-before-write text ported; mkdir via Host::create_dir)
├── edit.rs          # edit (9-strategy fuzzy matcher — the deep surface)
├── glob.rs          # glob (rg --files, gitignore-respecting, cap 100)
├── grep.rs          # grep (rg --json, --hidden, cap 100)
├── apply_patch.rs   # apply_patch (envelope parser + apply; shared with codex? §9 Q2)
├── descriptions/    # verbatim tool descriptions (provenance-pinned) — read/write/edit/glob/grep/apply_patch(.md); bash rendered
└── snapshots/       # byte-frozen rendered prompt goldens
```

## 3. Key types & schemas (verbatim from the survey; abbreviated here)
- **bash** (`shell/prompt.ts:15-23`): `command` (string, req, "The command to execute"),
  `timeout` (positive int, opt, "Optional timeout in milliseconds"), `workdir` (string,
  opt, "The working directory to run the command in. Defaults to the current directory.
  Use this instead of 'cd' commands.").
- **read** (`read.ts:28-36`): `filePath` (string, req), `offset` (nonneg int, opt),
  `limit` (nonneg int, opt) — verbatim field descriptions in the survey.
- **write** (`write.ts:20-25`): `content` (string, req), `filePath` (string, req).
- **edit** (`edit.ts:47-56`): `filePath`, `oldString`, `newString` (all req),
  `replaceAll` (bool, opt).
- **glob** (`glob.ts:10-15`): `pattern` (string, req), `path` (string, opt — the same
  "IMPORTANT: Omit this field … DO NOT enter 'undefined'/'null'" sentence claude/CC use).
- **grep** (`grep.ts:10-18`): `pattern` (string, req), `path` (string, opt), `include`
  (string, opt).
- **apply_patch** (`apply_patch.ts:18-20`): `patchText` (string, req).
All Args `#[serde(deny_unknown_fields)]` unless the source is lenient (opencode uses
Effect `Schema`; confirm strictness per tool at implementation). Type-strict decoding
(repo policy) for numeric/bool fields.

## 4. Behavior / algorithms (faithful caps + guardrails — from the survey)
- **bash** (`shell.ts`): default timeout **120000 ms** (`bashDefaultTimeoutMs ?? 2min`),
  negative rejected; kill at `timeout+100ms`, force-kill after 3 s; output truncation
  2000 lines / 50 KB (`truncate.ts:15-16`), **tail**-truncated with a
  `...output truncated...\nFull output saved to: <file>` prefix; empty output →
  `(no output)`; `<shell_metadata>` block on timeout/abort. Tree-sitter permission
  analysis is engine/approval machinery — **not** ported (the tool surface = name + 3
  args + description). Runs via `Host::exec`.
- **read** (`read.ts`): default 2000 lines; per-line 2000-char cap with
  `... (line truncated to 2000 chars)`; 50 KB byte cap; output = `<path>…</path>\n
  <type>file</type>\n<content>\n` + each line `${i+offset}: ${line}` + a trailer
  (`(Output capped …)` / `(Showing lines a-b of N …)` / `(End of file - total N lines)`)
  + `</content>`. **Directory branch**: lists entries (dirs get `/`, `localeCompare`
  sort) wrapped in `<type>directory</type><entries>`. Binary detection → "Cannot read
  binary file". Images/PDF → attachments (deferred, text-only v0). No in-tool
  read-before-edit store.
- **write** (`write.ts`): overwrite; **create parent dirs** (→ `Host::create_dir`);
  BOM handling; output `Wrote file successfully.` The read-before-write rule is in
  `write.txt` (description) but enforced engine-side — **not** an in-tool gate (unlike
  claude). LSP tails/permission asks are engine seams (dropped).
- **edit** (`edit.ts`): `oldString===newString` → `No changes to apply: …`; empty
  `oldString` = create-if-absent (else `oldString cannot be empty when editing an
  existing file…`); line-ending detect/normalize/restore; BOM; **9 replacer strategies
  in order** (`Simple, LineTrimmed, BlockAnchor, WhitespaceNormalized,
  IndentationFlexible, EscapeNormalized, TrimmedBoundary, ContextAware,
  MultiOccurrence`; BlockAnchor/ContextAware use Levenshtein ≥0.65);
  `isDisproportionateMatch` guard; non-`replaceAll` uniqueness (indexOf≠lastIndexOf →
  "Found multiple matches …"); not-found → "Could not find oldString …"; output
  `Edit applied successfully.` **Note:** `edit.txt`'s stated error strings differ from
  the code's thrown strings — we port `edit.txt` verbatim (description) and mirror the
  code's runtime strings for behavior (document the divergence).
- **glob** (`glob.ts` → `ripgrep.ts:155-168`): `rg --no-config --files --glob=<pattern>
  --glob=!**/.git/** .` — **respects .gitignore, excludes hidden** (contrast claude's
  `--no-ignore --hidden`); cap **100**; truncation note when `length===limit`; absolute
  paths; ripgrep `--files` order (no mtime sort).
- **grep** (`grep.ts` → `ripgrep.ts:218-229`): `rg --no-config --json --hidden
  --no-messages [--glob=<include>] --glob=!**/.git/** -- <pattern>` — **passes
  `--hidden`** (diverges from glob); respects .gitignore; cap 100; output `Found N
  matches[ (more matches available)]` + per-file `path:` blocks with `  Line N: text`.
- **apply_patch** (`apply_patch.ts`): parse the `*** Begin Patch / *** End Patch`
  envelope (Add/Update/Delete/Move); empty → `patch rejected: empty patch`; parse fail
  → `apply_patch verification failed: <err>`; output `Success. Updated the following
  files:` with `A/M/D` prefixes. **Coordinate the parser with the codex pack's
  apply_patch** — same envelope family; share the parser module if the formats match
  (verify at implementation — opencode's `Patch.parsePatch` vs codex's parser).

## 5. Design decisions (each: source `file:line` · why · why-not · difference)
_(Filled per-tool at implementation, mirroring the claude plan §5 discipline. The
load-bearing ones are the open questions in §9.)_
- 7-tool subset (vs grok 5 / claude 6 / codex 2) — the tool-surface A/B axis.
- Faithful 9-strategy fuzzy edit — opencode's real behavior; the type-strict policy is
  about *arg decoding*, not about flattening a harness's intentional fuzzy matcher.
- Descriptions verbatim from `.txt` (opencode = the pure-static description model).
- glob/grep hidden-file asymmetry reproduced verbatim (it's opencode's real quirk).

## 6. Tests
Schema goldens (7 specs, verbatim field descriptions, `additionalProperties`);
description provenance pins (`descriptions/*.md` len + sha256 + opener); behavior via
`build_registry`+`dispatch` over a tempdir host — read format + dir branch + caps;
write create+mkdir; **edit: one test per replacer strategy** + disproportion guard +
uniqueness + create-if-absent; glob/grep (rg-gated) incl. the hidden-file asymmetry;
apply_patch add/update/delete/move + empty/parse-fail; prompt snapshots (selected
base prompt + dynamic env block) + `strip_identity` + no-`{{`-token assert.

## 7. Dependencies to add
**None expected** — rg via `Host::run_capture`; parser std-only; Levenshtein hand-rolled
or a tiny inline impl (avoid a dep — confirm at implementation). Flag `ask-first` if the
9-strategy matcher genuinely needs a fuzzy-match crate.

## 8. Proposed ADR/SPEC deltas (apply at implementation time)
- **ADR-0012 dated amendment (Task 15):** opencode pack scope — 7 tools incl. the
  model-gated apply_patch⟷edit/write visibility (per §9 Q2's resolution) and the
  9-strategy fuzzy edit; per-provider prompt selection.
- **SPEC:** mark `opencode` pack landing; the remaining `locode` best-of pack is the
  last milestone.

## 9. Open questions (for user sign-off before implementation)
1. **bash description is DYNAMIC** (4 shell profiles bash/pwsh/windows-powershell/cmd ×
   OS × interpolated tmp/timeout/limits, `shell/prompt.ts:78-291`). A static `&str`
   can't hold all four. **Proposal:** render the **bash/unix profile** with our defaults
   baked in (120000 ms, 2000 lines, 50 KB), store it as a pinned description, and log
   the dropped PowerShell/cmd variants as a gap — exactly how the claude pack resolved
   Bash's conditionals at port time (`docs/research/tool-description-interface.md`).
   Confirm.
2. **apply_patch ⟷ edit/write model-gating (§1.2).** Options: (a) expose all 7 always
   (max surface, simplest, but not what opencode shows any single model); (b) reproduce
   the `usePatch` gate via `PackContext.model` (faithful — a GPT model sees apply_patch
   only; others see edit+write) — tool surface varies per run; (c) pick one config
   (e.g. always edit+write, apply_patch off) and document. **Recommendation: (b)** — it
   is the faithful behavior and we now have `PackContext.model`; note it in the report.
   Confirm.
3. **Which base prompt.** opencode selects a per-provider static `.txt` by model-id
   substring (`session/system.ts:27-42`): `anthropic.txt`/`gpt.txt`/`codex.txt`/
   `gemini.txt`/`default.txt`/… — all self-identify as "OpenCode" and contain
   loop-adjacent instructions (TodoWrite/Task sections). **Proposal:** reproduce
   opencode's selection from `PackContext.model` (ship the matching `.txt`; `default.txt`
   when unmatched), port the text **as-is** (machinery to the engine — the fidelity
   boundary), and pin snapshots. Confirm — or fix one prompt regardless of model?
4. **9-strategy fuzzy edit — confirm we port the fuzziness faithfully.** It is the
   opposite of grok/claude exact-match and of our type-strict *arg* policy, but it IS
   opencode's real edit behavior (the A/B signal). Big implementation surface (~9
   matchers + Levenshtein + disproportion guard). Confirm full port (vs a reduced-fidelity
   subset with a documented gap).
5. **Read-before-edit is NOT an in-tool gate here** (unlike claude): opencode states the
   rule in `edit.txt`/`write.txt` but enforces it engine-side over message history
   (loop-adjacent). **Proposal:** port the description text; do NOT add a freshness store
   — the enforcement is engine machinery, note the seam. Confirm.
6. **apply_patch parser sharing with the codex pack** — same envelope family? If the
   formats match, share `locode-packs::apply_patch`; if they diverge, per-pack parsers.
   Decide at implementation after diffing both parsers (pre-authorize following source).

## 10. Result
_(filled at merge — Phase 4)_
